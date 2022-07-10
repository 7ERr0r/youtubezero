use crate::ytzero::IsAudioVideo;
use crate::ytzero::SessionConsts;
use crate::zeroerror::Result;
use crate::zeroerror::YoutubezeroError;
use bytes::Bytes;
use core::time::Duration;
use error_chain::ChainedError;
use std::time::Instant;
use tokio::task::JoinHandle;

use std::cell::RefCell;
use std::rc::Rc;

use crate::model;
use crate::outwriter;
use crate::stderr;
use crate::stderr_result;
use crate::youtube;
use tokio::sync::mpsc;
use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::Sender;

pub enum NextStep {
    Download(reqwest::Response),
    Retry,
    RetryRedirect(String),
    Stop,
}

pub enum SegmentBytes {
    EOF,
    More(Bytes),
}

pub struct Segment {
    pub sq: RefCell<i64>,
    pub rx: RefCell<Receiver<SegmentBytes>>,

    // Not implemented yet
    pub requested_head: bool,
}

pub async fn start_download_joiner(
    client: reqwest::Client,
    mut txbufs: Vec<outwriter::OutputStreamSender>,
    format: Rc<RefCell<model::AdaptiveFormat>>,
    consts: Rc<SessionConsts>,
    isav: IsAudioVideo,
    copy_ended: Rc<RefCell<bool>>,
) -> Result<()> {
    let (segments_tx, mut segments_rx) = mpsc::channel::<Rc<Segment>>(200);

    stderr!(
        "{} Downloading: {} - {} - {:?} bit/s, {}\n",
        isav,
        consts.player.videoDetails.title,
        consts.video_id,
        format.borrow().bitrate,
        format.borrow().mimeType
    );

    let join_handle =
        spawn_unordered_download_format(client, format, consts.clone(), segments_tx, isav);

    while !*copy_ended.borrow() {
        let one_segment_future =
            join_one_segment(&mut segments_rx, consts.clone(), &mut txbufs, isav);

        match tokio::time::timeout(Duration::from_secs(100), one_segment_future).await {
            Err(_) => {
                stderr!("\n{} ordered_download: segment timed out\n\n", isav);
            }
            Ok(Err(err)) => {
                return Err(err);
            }
            Ok(Ok(JoinSegmentResult::GoNext)) => {
                // we got the whole segment
            }
            Ok(Ok(JoinSegmentResult::Eof)) => {
                drop(txbufs);
                stderr!("\n{} ordered_download: JoinSegmentResult::Eof\n\n", isav);
                // end of the whole segment stream
                break;
            }
        }
    }
    if let Err(_err) = tokio::time::timeout(Duration::from_secs(1), join_handle).await {
        stderr!("\n{} unordered_download_format: timed out\n\n", isav);
    }
    Ok(())
}

enum JoinSegmentResult {
    GoNext,
    Eof,
}

// copies from Segment-s in segments_rx
// to channels of Vec<u8> in txbufs
async fn join_one_segment(
    segments_rx: &mut Receiver<Rc<Segment>>,
    consts: Rc<SessionConsts>,
    txbufs: &mut Vec<outwriter::OutputStreamSender>,
    isav: IsAudioVideo,
) -> Result<JoinSegmentResult> {
    let res = segments_rx.recv().await;
    match res {
        None => {
            //stderr!("\n{} ordered_download: no more segments\n", isav);
            //return Err(format!("ordered_download: no more segments {}", isav).into());
            return Ok(JoinSegmentResult::Eof);
        }
        Some(segment) => {
            {
                let sq: i64 = *segment.sq.borrow();
                if consts.should_discard_sq(sq) {
                    stderr!("{} ignoring segment {}\n", isav, sq,);
                    return Ok(JoinSegmentResult::GoNext);
                } else {
                    stderr!("{} <- joining segment {}\n", isav, sq,);
                }
            }

            loop {
                let rx = &segment.rx;
                let mut rx = rx.borrow_mut();

                let out_segment_bytes = rx.recv().await;

                match out_segment_bytes {
                    None => {
                        // Sender channel dropped
                        break;
                    }
                    Some(SegmentBytes::EOF) => {
                        break;
                    }
                    Some(SegmentBytes::More(bytes)) => {
                        for out in txbufs.iter_mut() {
                            //stderr!("x")?;
                            // looks like with youtube we can't skip bytes...
                            // :(
                            if out.reliable {
                                out.tx
                                    .send(bytes.clone())
                                    .await
                                    .map_err(|_| YoutubezeroError::SegmentJoinSendError)?;
                            } else {
                                // non-blocking
                                let _r = out.tx.try_send(bytes.clone());
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(JoinSegmentResult::GoNext)
}

async fn grab_tickets(
    num_grab: usize,
    rx_tickets: &mut Receiver<bool>,
    tx_tickets: &Sender<bool>,
    isav: IsAudioVideo,
    head_seqnum: &Rc<RefCell<i64>>,
) -> Result<()> {
    let tickets_start = Instant::now();
    let mut grabbed = 0;
    for _i in 0..num_grab {
        match tokio::time::timeout(Duration::from_millis(12000), rx_tickets.recv()).await {
            Err(_) => {
                break;
            }
            Ok(_) => {
                grabbed += 1;
            }
        }
    }
    for _i in 0..grabbed {
        let _ = tx_tickets.send(true).await;
    }
    stderr!(
        "{} waiting for ticket took {:04} ms ({} is head)\n",
        isav,
        tickets_start.elapsed().as_millis(),
        *head_seqnum.borrow()
    );
    Ok(())
}

async fn unordered_download_format(
    client: reqwest::Client,
    format: Rc<RefCell<model::AdaptiveFormat>>,
    isav: IsAudioVideo,
    consts: Rc<SessionConsts>,
    segments_tx: Sender<Rc<Segment>>,
) -> Result<()> {
    let is_low_latency: bool = consts.is_low_latency();
    let is_live_segmented = consts.player.videoDetails.isLive.unwrap_or_default();
    let max_over_head_diff: i64 = if is_low_latency { 2 } else { 1 };

    let mut current_seqnum: i64 = consts.start_seqnum;
    let mut is_first = true;
    let initial_head = if is_live_segmented {
        -1
    } else {
        let content_len = format.borrow().content_length.unwrap_or_default();
        content_len / consts.fake_segment_size
    };
    let head_seqnum: Rc<RefCell<i64>> = Rc::new(RefCell::new(initial_head));

    let max_in_flight = consts.max_in_flight;
    let (tx_tickets, mut rx_tickets) = mpsc::channel::<bool>(max_in_flight);

    for _i in 0..max_in_flight {
        drop(tx_tickets.try_send(true));
    }

    loop {
        let _ = rx_tickets.recv().await;

        {
            let head: i64 = *head_seqnum.borrow();

            let diff = current_seqnum - head;

            if diff >= max_over_head_diff {
                let grab_invalid_tickets = max_in_flight - max_over_head_diff as usize;
                grab_tickets(grab_invalid_tickets, &mut rx_tickets, &tx_tickets, isav, &head_seqnum).await?;
            }
            if consts.follow_head_seqnum {
                if diff < -20 {
                    current_seqnum = head;
                    stderr!("{} set seqnum to head: {}\n", isav, head);
                }
            }
        }

        let chan_len = if consts.whole_segments { 2 } else { 512 };
        let (tx, rx) = mpsc::channel::<SegmentBytes>(chan_len);

        // TODO: maybe set requested_head if is_first
        // and set header ("headm", "3")
        let _ = is_first;
        let segment = Rc::new(Segment {
            requested_head: false,
            sq: RefCell::new(current_seqnum),
            //tx: RefCell::new(Some(tx)),
            rx: RefCell::new(rx),
        });

        segments_tx
            .send(segment.clone())
            .await
            .map_err(|_| YoutubezeroError::SegmentSendError)?;
        let join_handle = {
            let client_clone = client.clone();
            let format = format.clone();
            let consts = consts.clone();
            let tx_tickets = tx_tickets.clone();
            let head_seqnum = head_seqnum.clone();
            let join_handle = tokio::task::spawn_local(async move {
                let download_future = youtube::download_format_segment_retrying(
                    client_clone,
                    segment.clone(),
                    tx,
                    format,
                    isav,
                    consts,
                    head_seqnum,
                );

                match download_future.await {
                    Err(err) => {
                        let _ =
                            stderr_result!("{} fetch_segment err: {}\n", isav, err.display_chain());
                        // network/http problems
                        tokio::time::sleep(Duration::from_millis(2000)).await;
                    }
                    Ok(()) => {
                        //let _ = stderr!("{}new_head_seqnum: {}\n", prefix, new_head_seqnum);
                        //*(head_seqnum.borrow_mut()) = new_head_seqnum;
                    }
                }
                let _ = tx_tickets.send(true).await;
            });
            join_handle
        };

        if let Some(end_seqnum) = consts.end_seqnum {
            if current_seqnum == end_seqnum {
                join_handle.await?;
                break;
            }
        }

        if !is_live_segmented && current_seqnum == initial_head {
            join_handle.await?;
            break;
        }

        if current_seqnum == 0 && consts.follow_head_seqnum {
            // wait for first head-seqnum

            let _ = tokio::time::timeout(Duration::from_millis(5000), join_handle).await;
        } else {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        current_seqnum += 1;
        is_first = false;
    }
    Ok(())
}

fn spawn_unordered_download_format(
    client: reqwest::Client,
    format: Rc<RefCell<model::AdaptiveFormat>>,
    consts: Rc<SessionConsts>,
    segments_tx: Sender<Rc<Segment>>,
    isav: IsAudioVideo,
) -> JoinHandle<()> {
    tokio::task::spawn_local(async move {
        let res = unordered_download_format(client, format, isav, consts, segments_tx).await;
        match res {
            Err(err) => {
                let _ = stderr_result!("{} download_format err: {}\n", isav, err);
            }
            _ => {}
        }
    })
}
