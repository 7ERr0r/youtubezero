use crate::ytzero::IsAudioVideo;
use crate::ytzero::SessionConsts;
use crate::zeroerror::Result;
use crate::zeroerror::YoutubezeroError;
use bytes::Bytes;
use core::time::Duration;
use error_chain::ChainedError;
use std::collections::BTreeMap;
use std::collections::VecDeque;
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


pub struct Segment {
    pub sq: i64,

    pub fixed_sq: RefCell<i64>,

    // Not implemented yet
    pub requested_head: bool,
}

#[derive(Default)]
pub struct SegmentsBuf {
    pub chunks: VecDeque<Bytes>,
    pub eof: bool,
}

pub async fn start_download_joiner(
    client: reqwest::Client,
    mut txbufs: Vec<outwriter::OutputStreamSender>,
    format: Rc<RefCell<model::AdaptiveFormat>>,
    consts: Rc<SessionConsts>,
    isav: IsAudioVideo,
    copy_ended: Rc<RefCell<bool>>,
    mut event_rx: Receiver<OrderingEvent>,
    event_tx: Sender<OrderingEvent>,
) -> Result<()> {
    stderr!(
        "{} Downloading: {} - {} - {:?} bit/s, {}\n",
        isav,
        consts.player.videoDetails.title,
        consts.video_id,
        format.borrow().bitrate,
        format.borrow().mimeType
    );

    let mut segment_map: BTreeMap<i64, SegmentsBuf> = Default::default();

    let join_handle =
        spawn_unordered_download_format(client, format, consts.clone(), isav, event_tx);

    while !*copy_ended.borrow() {

        let segment_event = event_rx.recv().await;
        if let Some(event) = segment_event {
            handle_single_event(event, &consts, isav, &mut segment_map).await?;
        } else {
            break;
        }
        process_first_buf_element(&consts, &mut txbufs, isav, &mut segment_map).await?;
    }
    if let Err(_err) = tokio::time::timeout(Duration::from_secs(1), join_handle).await {
        stderr!("\n{} unordered_download_format: timed out\n\n", isav);
    }
    Ok(())
}


async fn handle_single_event(
    segment_event: OrderingEvent,
    consts: &Rc<SessionConsts>,
    isav: IsAudioVideo,
    segment_map: &mut BTreeMap<i64, SegmentsBuf>,
) -> Result<()> {
    match segment_event {
        OrderingEvent::SegmentStart(sq) => {
            if consts.should_discard_sq(sq) {
                stderr!("{} ignoring sq={}\n", isav, sq,);
                //return Ok(JoinSegmentResult::GoNext);
            } else {
                stderr!("{} SegmentStart sq={}\n", isav, sq,);
                segment_map.insert(sq, SegmentsBuf::default());
            }
        }
        OrderingEvent::SegmentEof(sq) => {
            let buf = segment_map.get_mut(&sq);
            if let Some(buf) = buf {
                buf.eof = true;
            }
        }

        OrderingEvent::SegmentData(sq, chunk) => {
            let buf = segment_map.get_mut(&sq);
            if let Some(buf) = buf {
                buf.chunks.push_back(chunk);
            }
        }
    }
    Ok(())
}

async fn process_first_buf_element(
    _consts: &Rc<SessionConsts>,
    txbufs: &mut Vec<outwriter::OutputStreamSender>,
    isav: IsAudioVideo,
    segment_map: &mut BTreeMap<i64, SegmentsBuf>,
) -> Result<()> {
    if let Some(entry) = segment_map.iter_mut().next() {
        let sq = *entry.0;
        //current_sq = Some(sq);

        let buf = entry.1;

        while let Some(chunk) = buf.chunks.pop_front() {
            stderr!(
                "{} writing buffered sq={} chunk len={}\n",
                isav,
                sq,
                chunk.len()
            );
            write_txbufs(txbufs, &chunk).await?;
        }
        if buf.eof {
            segment_map.remove(&sq);
        }
    }
    Ok(())
}

async fn write_txbufs(
    txbufs: &mut Vec<outwriter::OutputStreamSender>,
    message: &Bytes,
) -> Result<()> {
    for out in txbufs.iter_mut() {
        //stderr!("x")?;
        // looks like with youtube we can't skip bytes...
        // :(
        if out.reliable {
            out.tx
                .send(message.clone())
                .await
                .map_err(|_| YoutubezeroError::SegmentDataSendError)?;
        } else {
            // non-blocking
            let _r = out.tx.try_send(message.clone());
        }
    }
    Ok(())
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

pub enum OrderingEvent {
    SegmentStart(i64),
    SegmentData(i64, Bytes),
    SegmentEof(i64),
}

async fn unordered_download_format(
    client: reqwest::Client,
    format: Rc<RefCell<model::AdaptiveFormat>>,
    isav: IsAudioVideo,
    consts: Rc<SessionConsts>,
    event_tx: Sender<OrderingEvent>,
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
                grab_tickets(
                    grab_invalid_tickets,
                    &mut rx_tickets,
                    &tx_tickets,
                    isav,
                    &head_seqnum,
                )
                .await?;
            }
            if consts.follow_head_seqnum {
                if diff < -20 {
                    current_seqnum = head;
                    stderr!("{} set seqnum to head: {}\n", isav, head);
                }
            }
        }

        // TODO: maybe set requested_head if is_first
        // and set header ("headm", "3")
        let _ = is_first;
        let segment = Rc::new(Segment {
            requested_head: false,
            sq: current_seqnum,
            fixed_sq: RefCell::new(current_seqnum),
        });


        event_tx
            .send(OrderingEvent::SegmentStart(segment.sq))
            .await
            .map_err(|_| YoutubezeroError::SegmentEventSendError)?;

        let join_handle = {
            let client_clone = client.clone();
            let format = format.clone();
            let consts = consts.clone();
            let tx_tickets = tx_tickets.clone();
            let head_seqnum = head_seqnum.clone();
            let tx = event_tx.clone();
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
    isav: IsAudioVideo,
    event_tx: Sender<OrderingEvent>,
) -> JoinHandle<()> {
    tokio::task::spawn_local(async move {
        let res = unordered_download_format(client, format, isav, consts, event_tx).await;
        match res {
            Err(err) => {
                let _ = stderr_result!("{} download_format err: {}\n", isav, err);
            }
            _ => {}
        }
    })
}
