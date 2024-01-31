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
use std::time::Instant;
use tokio::task::spawn_local;
use tokio::task::JoinHandle;
use tokio::time::sleep;
use tokio::time::timeout;

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

    let join_handle =
        spawn_unordered_download_format(client, stream.clone(), state.clone(), event_tx);

    while !*copy_ended.borrow() {
        let segment_event = event_rx.recv().await;
        if let Some(event) = segment_event {
            handle_single_event(event, &stream, &state, &mut segment_map).await?;
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
    stream: &StreamDownloader,
    state: &Rc<RefCell<StreamState>>,
    segment_map: &mut BTreeMap<i64, SegmentsBuf>,
) -> Result<()> {
    match segment_event {
        OrderingEvent::HeadChange(sq) => {
            stderr!("{} ignoring sq={}\n", stream.isav, sq,);
            state.borrow_mut().head_seqnum = sq;
        }
        OrderingEvent::SegmentStart(sq) => {
            if stream.consts.should_discard_sq(sq) {
                stderr!("{} ignoring sq={}\n", stream.isav, sq,);
                //return Ok(JoinSegmentResult::GoNext);
            } else {
                stderr!("{} SegmentStart sq={}\n", stream.isav, sq,);
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

/// writes from to txbufs
async fn process_first_buf_element(
    _consts: &Rc<SessionConsts>,
    txbufs: &mut Vec<outwriter::OutputStreamSender>,
    isav: IsAudioVideo,
    segment_map: &mut BTreeMap<i64, SegmentsBuf>,
) -> Result<()> {
    if let Some((&sq, buf)) = segment_map.iter_mut().next() {
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
    state: Option<&Rc<RefCell<StreamState>>>,
) -> Result<()> {
    let tickets_start = Instant::now();
    let mut grabbed = 0;
    for _i in 0..num_grab {
        let duration = Duration::from_millis(12000);
        match timeout(duration, rx_tickets.recv()).await {
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
    if let Some(state) = state {
        stderr!(
            "{} waiting for ticket took {:04} ms ({} is head)\n",
            isav,
            tickets_start.elapsed().as_millis(),
            state.borrow().head_seqnum
        );
    }
    Ok(())
}

pub enum OrderingEvent {
    HeadChange(i64),
    SegmentStart(i64),
    SegmentData(i64, Bytes),
    SegmentEof(i64),
}

async fn unordered_download_format(
    client: reqwest::Client,
    stream: StreamDownloader,
    state: Rc<RefCell<StreamState>>,
    // aformat: Rc<RefCell<model::AdaptiveFormat>>,
    // consts: Rc<SessionConsts>,
    // isav: IsAudioVideo,
    event_tx: Sender<OrderingEvent>,
) -> Result<()> {
    let (tx_tickets, mut rx_tickets) = mpsc::channel::<bool>(stream.consts.max_in_flight);

    for _i in 0..stream.consts.max_in_flight {
        let _ = tx_tickets.try_send(true);
    }

    loop {
        let flow = unordered_download_tick(
            &client,
            &stream,
            &state,
            &tx_tickets,
            &mut rx_tickets,
            &event_tx,
        )
        .await?;
        if let DownloadControlFlow::Break = flow {
            break;
        }
    }
    Ok(())
}

enum DownloadControlFlow {
    Continue,
    Break,
}

#[derive(Clone)]
pub struct StreamDownloader {
    consts: Rc<SessionConsts>,
    aformat: Rc<RefCell<AdaptiveFormat>>,
    max_over_head_diff: i64,
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
        let max_over_head_diff: i64 = if is_low_latency { 2 } else { 1 };

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
}

impl StreamState {
    pub fn new<'a>(stream: &StreamDownloader) -> Rc<RefCell<StreamState>> {
        Rc::new(RefCell::new(StreamState {
            head_seqnum: stream.initial_head,
            current_seqnum: stream.consts.start_seqnum,
        }))
    }
}

async fn unordered_download_tick(
    client: &reqwest::Client,
    stream: &StreamDownloader,
    state: &Rc<RefCell<StreamState>>,

    tx_tickets: &Sender<bool>,
    rx_tickets: &mut Receiver<bool>,

    event_tx: &Sender<OrderingEvent>,
) -> Result<DownloadControlFlow> {
    let _ = rx_tickets.recv().await;

    {
        let head: i64 = state.borrow().head_seqnum;

        let diff = state.borrow().current_seqnum - head;

        if diff >= stream.max_over_head_diff {
            let grab_invalid_tickets =
                stream.consts.max_in_flight as usize - stream.max_over_head_diff as usize;
            grab_tickets(
                grab_invalid_tickets,
                rx_tickets,
                &tx_tickets,
                stream.isav,
                Some(state),
            )
            .await?;
        }
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

    event_tx
        .send(OrderingEvent::SegmentStart(segment.sq))
        .await
        .map_err(|_| YoutubezeroError::SegmentEventSendError)?;

    let join_handle = {
        let client_clone = client.clone();
        let aformat = stream.aformat.clone();
        let consts = stream.consts.clone();
        let tx_tickets = tx_tickets.clone();
        let state = state.clone();
        let tx = event_tx.clone();
        let isav = stream.isav.clone();
        let join_handle = tokio::task::spawn_local(async move {
            let download_future = youtube::download_format_segment_retrying(
                client_clone,
                segment.clone(),
                tx,
                aformat,
                isav,
                consts,
                state,
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
            let _ = tx_tickets.send(true).await;
        });
        join_handle
    };

    if let Some(end_seqnum) = stream.consts.end_seqnum {
        if state.borrow().current_seqnum == end_seqnum {
            join_handle.await?;
            return Ok(DownloadControlFlow::Break);
        }
    }

    let is_live_segmented = stream.consts.player.videoDetails.isLive.unwrap_or_default();
    if !is_live_segmented && state.borrow().current_seqnum == stream.initial_head {
        join_handle.await?;
        return Ok(DownloadControlFlow::Break);
    }

    if state.borrow().current_seqnum == 0 && stream.consts.follow_head_seqnum {
        // wait for first head-seqnum

        let _ = timeout(Duration::from_millis(5000), join_handle).await;
    } else {
        sleep(Duration::from_millis(20)).await;
    }
    state.borrow_mut().current_seqnum += 1;

    Ok(DownloadControlFlow::Continue)
}

fn spawn_unordered_download_format(
    client: reqwest::Client,
    // format: Rc<RefCell<model::AdaptiveFormat>>,
    // consts: Rc<SessionConsts>,
    // isav: IsAudioVideo,
    stream: StreamDownloader,
    state: Rc<RefCell<StreamState>>,
    event_tx: Sender<OrderingEvent>,
) -> JoinHandle<()> {
    spawn_local(async move {
        let isav = stream.isav;
        let res = unordered_download_format(client, stream, state, event_tx).await;
        match res {
            Err(err) => {
                let _ = stderr_result!("{} download_format err: {}\n", isav, err);
            }
            _ => {}
        }
    })
}
