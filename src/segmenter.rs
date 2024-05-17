use crate::model::AdaptiveFormat;
use crate::ytzero::IsAudioVideo;
use crate::ytzero::SessionConsts;
use crate::zeroerror::Result;
use crate::zeroerror::YoutubezeroError;
use bytes::Bytes;
use core::time::Duration;
use error_chain::ChainedError;
use std::collections::BTreeMap;
use std::collections::VecDeque;

use tokio::time::sleep;

use std::cell::RefCell;
use std::rc::Rc;

use crate::model;
use crate::outwriter;
use crate::stderr;
use crate::stderr_result;
use crate::youtube;
use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::Sender;

pub enum NextStep {
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
    aformat: Rc<RefCell<model::AdaptiveFormat>>,
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
        aformat.borrow().bitrate,
        aformat.borrow().mimeType
    );

    let mut segment_map: BTreeMap<i64, SegmentsBuf> = Default::default();

    let stream = StreamDownloader::new(isav, consts.clone(), aformat);
    let state = StreamState::new(&stream);

    // let join_handle =
    //     spawn_unordered_download_format(client, stream.clone(), state.clone(), event_tx);

    // first event inits
    state.borrow_mut().running_tasks += 1;
    let _ = event_tx.try_send(OrderingEvent::TaskStart);

    while !*copy_ended.borrow() {
        let segment_event = event_rx.recv().await;
        if let Some(event) = segment_event {
            handle_single_event(&client, event, &stream, &state, &mut segment_map, &event_tx)
                .await?;
        } else {
            break;
        }
        process_first_buf_elements(&consts, &mut txbufs, isav, &mut segment_map).await?;
    }
    // if let Err(_err) = tokio::time::timeout(Duration::from_secs(1), join_handle).await {
    //     stderr!("\n{} unordered_download_format: shutdown time out\n\n", isav);
    // }
    Ok(())
}

async fn handle_single_event(
    client: &reqwest::Client,
    segment_event: OrderingEvent,
    stream: &StreamDownloader,
    state: &Rc<RefCell<StreamState>>,
    segment_map: &mut BTreeMap<i64, SegmentsBuf>,
    event_tx: &Sender<OrderingEvent>,
) -> Result<()> {
    match segment_event {
        OrderingEvent::HeadChange(head) => {
            let old_head = state.borrow().head_seqnum;
            if old_head != head {
                stderr!("{} set head seqnum sq={}\n", stream.isav, head,);
                state.borrow_mut().head_seqnum = head;
            }
        }
        OrderingEvent::SegmentStart(sq) => {
            let discard = stream.consts.should_discard_sq(sq);
            stderr!(
                "{} SegmentStart sq={sq} {}\n",
                stream.isav,
                if discard { "discard=true" } else { "" }
            );
            if discard {
                //return Ok(JoinSegmentResult::GoNext);
            } else {
                segment_map.insert(sq, SegmentsBuf::default());
            }
        }
        OrderingEvent::SegmentEof(sq) => {
            stderr!("{} SegmentEof sq={sq}\n", stream.isav);
            let buf = segment_map.get_mut(&sq);
            if let Some(buf) = buf {
                buf.eof = true;
            }
        }

        OrderingEvent::SegmentData(sq, chunk) => {
            //stderr!("{} SegmentData sq={sq}\n", stream.isav);
            let buf = segment_map.get_mut(&sq);
            if let Some(buf) = buf {
                buf.chunks.push_back(chunk);
            }
        }
        OrderingEvent::TaskStart => {
            stderr!("{} TaskStart\n", stream.isav);
            unordered_download_spawn(
                &client, &stream, &state,
                // &tx_tickets,
                // &mut rx_tickets,
                &event_tx,
            )
            .await?;
            state.borrow_mut().current_seqnum += 1;
        }
        OrderingEvent::TaskEnd => {
            let running = state.borrow_mut().running_tasks;
            stderr!("{} TaskEnd running={running}\n", stream.isav);
            if running == 0 {
                return Err(YoutubezeroError::RunningTasksIsZeroBug.into());
            }
            state.borrow_mut().running_tasks -= 1;

            // dont spawn too many at first
            for _ in 0..3 {
                if state.borrow_mut().running_tasks >= stream.consts.max_in_flight {
                    break;
                }
                {
                    let head: i64 = state.borrow().head_seqnum;

                    let diff = state.borrow().current_seqnum - head;

                    if diff >= stream.max_over_head_diff as i64 {
                        if state.borrow_mut().running_tasks == 0 {
                            //return Err(YoutubezeroError::DeadlockFromHeadBug.into());
                            let state = state.clone();
                            let event_tx = event_tx.clone();
                            tokio::task::spawn_local(async move {
                                state.borrow_mut().running_tasks += 1;
                                let _ = event_tx
                                    .send(OrderingEvent::TaskStart)
                                    .await
                                    .map_err(|_| YoutubezeroError::SegmentEventSendError);
                            });
                        }
                        break;
                    }
                }

                state.borrow_mut().running_tasks += 1;
                event_tx
                    .try_send(OrderingEvent::TaskStart)
                    .map_err(|_| YoutubezeroError::SegmentEventSendError)?;
            }
        }

        OrderingEvent::EndOfStream => {}
    }
    Ok(())
}

/// writes from segment_map buffers to txbufs
async fn process_first_buf_elements(
    _consts: &Rc<SessionConsts>,
    txbufs: &mut Vec<outwriter::OutputStreamSender>,
    isav: IsAudioVideo,
    segment_map: &mut BTreeMap<i64, SegmentsBuf>,
) -> Result<()> {
    if let Some((&sq, buf)) = segment_map.iter_mut().next() {
        while let Some(chunk) = buf.chunks.pop_front() {
            if false {
                stderr!(
                    "{} writing buffered sq={} chunk len={}\n",
                    isav,
                    sq,
                    chunk.len()
                );
            }
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

pub enum OrderingEvent {
    HeadChange(i64),
    SegmentStart(i64),
    SegmentData(i64, Bytes),
    SegmentEof(i64),

    TaskStart,
    TaskEnd,

    EndOfStream,
}

#[derive(Clone)]
pub struct StreamDownloader {
    consts: Rc<SessionConsts>,
    aformat: Rc<RefCell<AdaptiveFormat>>,
    max_over_head_diff: u8,
    initial_head: i64,
    isav: IsAudioVideo,
}
impl StreamDownloader {
    pub fn new(
        isav: IsAudioVideo,
        consts: Rc<SessionConsts>,
        aformat: Rc<RefCell<AdaptiveFormat>>,
    ) -> StreamDownloader {
        let is_low_latency: bool = consts.is_low_latency();
        let max_over_head_diff: u8 = if is_low_latency { 2 } else { 1 };

        let initial_head = if consts.player.videoDetails.isLive.unwrap_or_default() {
            -1
        } else {
            let content_len = aformat.borrow().content_length.unwrap_or_default();
            content_len / consts.fake_segment_size
        };

        StreamDownloader {
            consts: consts,
            aformat: aformat,
            max_over_head_diff,
            initial_head,
            isav,
        }
    }
}

#[derive(Debug)]
pub struct StreamState {
    pub current_seqnum: i64,
    pub head_seqnum: i64,

    pub running_tasks: u16,
}

impl StreamState {
    pub fn new<'a>(stream: &StreamDownloader) -> Rc<RefCell<StreamState>> {
        Rc::new(RefCell::new(StreamState {
            head_seqnum: stream.initial_head,
            current_seqnum: stream.consts.start_seqnum,
            running_tasks: 0,
        }))
    }
}

async fn unordered_download_spawn(
    client: &reqwest::Client,
    stream: &StreamDownloader,
    state: &Rc<RefCell<StreamState>>,

    // tx_tickets: &Sender<bool>,
    // rx_tickets: &mut Receiver<bool>,
    event_tx: &Sender<OrderingEvent>,
) -> Result<()> {
    //let _ = rx_tickets.recv().await;

    {
        let head: i64 = state.borrow().head_seqnum;

        let diff = state.borrow().current_seqnum - head;
        if stream.consts.follow_head_seqnum {
            if diff < -20 {
                state.borrow_mut().current_seqnum = head;
                stderr!(
                    "{} set current_seqnum to head_seqnum: {}\n",
                    stream.isav,
                    head
                );
            }
        }
    }

    // TODO: maybe set requested_head if is_first
    // and set header ("headm", "3")

    let segment = Rc::new(Segment {
        requested_head: false,
        sq: state.borrow().current_seqnum,
        fixed_sq: RefCell::new(state.borrow().current_seqnum),
    });

    // Schedule next segment
    event_tx
        .try_send(OrderingEvent::SegmentStart(segment.sq))
        .map_err(|_| YoutubezeroError::SegmentEventSendError)?;

    let join_handle = {
        let client_clone = client.clone();
        let aformat = stream.aformat.clone();
        let consts = stream.consts.clone();
        //let tx_tickets = tx_tickets.clone();
        let state = state.clone();
        let tx = event_tx.clone();
        let isav = stream.isav.clone();
        let stream = stream.clone();
        let join_handle = tokio::task::spawn_local(async move {
            let download_future = youtube::download_format_segment_retrying(
                client_clone,
                segment.clone(),
                tx.clone(),
                aformat,
                isav,
                consts,
                state.clone(),
            );

            match download_future.await {
                Err(err) => {
                    let _ = stderr_result!("{} fetch_segment err: {}\n", isav, err.display_chain());
                    // network/http problems
                    tokio::time::sleep(Duration::from_millis(2000)).await;
                }
                Ok(()) => {
                    //let _ = stderr!("{}new_head_seqnum: {}\n", prefix, new_head_seqnum);
                    //*(head_seqnum.borrow_mut()) = new_head_seqnum;
                }
            }
            //let _ = tx_tickets.send(true).await;

            if let Some(end_seqnum) = stream.consts.end_seqnum {
                if state.borrow().current_seqnum == end_seqnum {
                    //join_handle.await?;
                    //return Ok(DownloadControlFlow::Break);
                    let _ = tx
                        .send(OrderingEvent::EndOfStream)
                        .await
                        .map_err(|_| YoutubezeroError::SegmentEventSendError);
                }
            }

            let is_live_segmented = stream.consts.player.videoDetails.isLive.unwrap_or_default();
            if !is_live_segmented && state.borrow().current_seqnum == stream.initial_head {
                //join_handle.await?;
                //return Ok(DownloadControlFlow::Break);
                let _ = tx
                    .send(OrderingEvent::EndOfStream)
                    .await
                    .map_err(|_| YoutubezeroError::SegmentEventSendError);
            }

            if state.borrow().current_seqnum == 0 && stream.consts.follow_head_seqnum {
                // // wait for first head-seqnum

                // let _ = timeout(Duration::from_millis(5000), join_handle).await;
            } else {
                sleep(Duration::from_millis(20)).await;
            }

            let _ = tx
                .send(OrderingEvent::TaskEnd)
                .await
                .map_err(|_| YoutubezeroError::SegmentEventSendError);
        });
        join_handle
    };
    Ok(())
}
