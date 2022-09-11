use crate::segmenter::OrderingEvent;
use crate::stderr;
use bytes::Bytes;
use tokio::sync::mpsc;
use tokio::sync::mpsc::Receiver;
use std::convert::TryFrom;

use crate::extras::ytsigurlfix::fix_format_url_sig_n;
use crate::macros::HexSlice;
use crate::model::AudioVideo;
use crate::youtube::PlayerResponseSource;
use crate::YoutubezeroArgs;
use core::time::Duration;
use std::path::PathBuf;

use crate::zeroerror::Result;
use crate::zeroerror::ResultExt;
use error_chain::ChainedError;
use futures::future::join_all;
use futures::future::select_all;

use crate::model;
use crate::youtube;
use futures::future::FutureExt;
use std::cell::RefCell;
use std::rc::Rc;

use crate::outwriter;
use crate::segmenter;

pub async fn run_async(args: &YoutubezeroArgs) -> Result<()> {
    let client = reqwest::Client::builder()
        .user_agent(youtube::MY_USER_AGENT)
        .build()
        .expect("should be able to build reqwest client");

    let watchv_source = PlayerResponseSource::try_from(args.url.as_str())?;
    stderr!("provided video_id: {:?}\n", watchv_source);
    
    let (player_response, opt_base_js_url) =
        youtube::fetch_player_response(&client, &watchv_source)
            .await
            .chain_err(|| "youtube::fetch_player_response")?;

    //stderr!("player_response: {:?}\n", player_response)?;

    let mut chosen;
    if let Some(ref streaming_data) = player_response.streamingData {
        let formats = &streaming_data.adaptiveFormats;
        for format in formats {
            stderr!("{}\n", format);
        }
        let option_chosen = model::get_choose_formats(
            formats,
            args.aformat.parse().ok(),
            args.vformat.parse().ok(),
        );
        chosen = option_chosen.audio_video_or_err()?;
        stderr!("chosen audio: {}\n", chosen.audio);
        stderr!("chosen video: {}\n", chosen.video);
    } else {
        let ps = player_response.playabilityStatus.unwrap_or_default();
        let msg = format!(
            "streamingData field not found: {} - {}",
            ps.status.unwrap_or_default(),
            ps.reason.unwrap_or_default()
        );
        return Err(msg.into());
    }

    chosen.audio.fix_content_length();
    chosen.video.fix_content_length();
    let base_js_bytes = youtube::maybe_get_base_js(&client, opt_base_js_url)
        .await
        .unwrap_or_default();
    drop(client);

    run_with_av_format(args, chosen, base_js_bytes, player_response).await?;
    Ok(())
}

pub async fn run_with_av_format(
    args: &YoutubezeroArgs,
    mut chosen: AudioVideo,
    base_js_bytes: Option<Bytes>,
    player_response: model::PlayerResponse,
) -> Result<()> {
    let mut handles: Vec<_> = Vec::new();
    handles.push(fix_format_url_sig_n(
        IsAudioVideo::audio(),
        &mut chosen.audio,
        base_js_bytes.clone(),
    ));
    handles.push(fix_format_url_sig_n(
        IsAudioVideo::video(),
        &mut chosen.video,
        base_js_bytes,
    ));

    for result in join_all(handles).await {
        if let Err(err) = result {
            stderr!(
                "warn: couldn't fix fix_format_url with JavaScript token generator: {}\n",
                err.display_chain().to_string()
            );
        }
    }

    let video_id = player_response.videoDetails.videoId.clone();
    let mut segment_dir = PathBuf::from(".");
    segment_dir.push("segments");
    segment_dir.push(format!("{}", HexSlice::new(&video_id)));

    let aud_segment_dir = Some({
        let mut p = segment_dir.clone();
        p.push("aud");
        p
    });
    let vid_segment_dir = Some({
        let mut p = segment_dir.clone();
        p.push("vid");
        p
    });

    let consts = Rc::new(SessionConsts {
        video_id: video_id,
        cpn: youtube::gen_cpn(),
        follow_head_seqnum: args.follow_head_seqnum,
        player: player_response,
        start_seqnum: args.start_seqnum,
        end_seqnum: args.end_seqnum,
        retries: args.retries,

        max_in_flight: args.max_in_flight,
        request_number: RefCell::new(1),
        timeout_one_request: args.timeout_one_request,

        whole_segments: args.whole_segments,
        cache_segments: args.cache_segments,
        aud_segment_dir,
        vid_segment_dir,
        truncate_output_files: args.truncate,

        fake_segment_size: 128 * 1024,

        
    });

    let local = tokio::task::LocalSet::new();
    // Run the on local task set.
    // This allows us to use Rc and RefCell
    // so that Mutex and Arc can be forgotten
    local
        .run_until(start_wait_audio_video(args, chosen, consts))
        .await?;
    tokio::time::sleep(Duration::from_millis(200)).await;

    Ok(())
}

async fn start_wait_audio_video(
    args: &YoutubezeroArgs,
    chosen: AudioVideo,
    consts: Rc<SessionConsts>,
) -> Result<()> {
    let client = reqwest::Client::builder()
        .user_agent(youtube::MY_USER_AGENT)
        .build()
        .expect("should be able to build reqwest client");
    let copy_ended = Rc::new(RefCell::new(false));

    let video_format = Rc::new(RefCell::new(chosen.video));
    let audio_format = Rc::new(RefCell::new(chosen.audio));

    let mut raw_futs = Vec::new();

    if args.vout.len() == 0 && args.aout.len() == 0 {
        stderr!("warn: zero audio and video outputs\n");
    }
    if args.vout.len() > 0 {
        let f1video = start_ordered_download(
            IsAudioVideo::video(),
            client.clone(),
            &args.vout,
            copy_ended.clone(),
            video_format,
            consts.clone(),
        )
        .fuse();
        raw_futs.push(f1video);
    }

    if args.aout.len() > 0 {
        let f2audio = start_ordered_download(
            IsAudioVideo::audio(),
            client.clone(),
            &args.aout,
            copy_ended.clone(),
            audio_format,
            consts,
        )
        .fuse();
        raw_futs.push(f2audio);
    }

    let unpin_futs: Vec<_> = raw_futs.into_iter().map(Box::pin).collect();
    let mut futs = unpin_futs;
    while !futs.is_empty() {
        match select_all(futs).await {
            (Ok(_), _index, remaining) => {
                futs = remaining;
            }
            (Err(err), _index, remaining) => {
                // Ignoring all errors
                let _futs = remaining;
                return Err(err);
            }
        }
    }

    let res: Result<()> = Ok(());
    res
}

async fn start_ordered_download(
    isav: IsAudioVideo,
    client: reqwest::Client,
    out_names: &[String],
    copy_ended: Rc<RefCell<bool>>,
    format: Rc<RefCell<model::AdaptiveFormat>>,
    consts: Rc<SessionConsts>,
) -> Result<()> {
    let chan_len = 1024 * 16;
    let (event_tx, event_rx) = mpsc::channel::<OrderingEvent>(chan_len);
    

    let vouts = outwriter::make_outs(
        isav,
        out_names,
        copy_ended.clone(),
        Some(&consts),
        Some(&format.borrow()),
    )
    .await?;
    let (txbufs, join_handles) = outwriter::make_out_writers(isav, vouts, copy_ended.clone());
    // tokio::task::spawn_local(async move {
    // })
    // .await??;

    segmenter::start_download_joiner(client, txbufs, format, consts, isav, copy_ended, event_rx, event_tx).await?;

    for out_handle in join_handles {
        let _ = tokio::time::timeout(Duration::from_secs(2), out_handle).await;
    }

    Ok(())
}

#[derive(Default)]
pub struct SessionConsts {
    pub cpn: String,
    pub video_id: String,
    pub follow_head_seqnum: bool,
    pub start_seqnum: i64,
    pub end_seqnum: Option<i64>,
    pub retries: usize,
    pub player: model::PlayerResponse,

    pub max_in_flight: usize,
    pub request_number: RefCell<i64>,
    pub timeout_one_request: Duration,

    pub whole_segments: bool,
    pub cache_segments: bool,
    pub aud_segment_dir: Option<PathBuf>,
    pub vid_segment_dir: Option<PathBuf>,
    pub truncate_output_files: bool,

    // for offline videos
    pub fake_segment_size: i64,
}

impl SessionConsts {
    pub fn should_discard_sq(&self, sq: i64) -> bool {
        let is_live_segmented = self.player.videoDetails.isLive.unwrap_or_default();
        sq < self.start_seqnum
            || (sq == 0 && self.follow_head_seqnum && is_live_segmented)
            || self.end_seqnum.map(|end| sq > end).unwrap_or_default()
    }
    // Not needed but let's pretend we're counting requests
    pub fn next_request_number(&self) -> i64 {
        let mut rn = self.request_number.borrow_mut();
        let current = *rn;
        *rn += 1;
        current
    }

    pub fn segment_path(&self, sq: i64, isav: IsAudioVideo) -> Option<PathBuf> {
        let dir = if isav.is_audio() {
            self.aud_segment_dir.as_ref()
        } else {
            self.vid_segment_dir.as_ref()
        };
        dir.map(|dir| {
            let mut fpath = dir.clone();
            fpath.push(format!("seg{}.ts", sq));
            fpath
        })
    }
    pub fn is_low_latency(&self) -> bool {
        self.player
            .videoDetails
            .isLowLatencyLiveStream
            .unwrap_or_default()
    }
    pub fn is_ultra_low_latency(&self) -> bool {
        self.player
            .videoDetails
            .latencyClass
            .as_ref()
            .map(|lc| lc == "MDE_STREAM_OPTIMIZATIONS_RENDERER_LATENCY_ULTRA_LOW")
            .unwrap_or_default()
    }
}

#[derive(Copy, Clone, Default, PartialEq, Eq)]
pub struct IsAudioVideo(bool);

impl IsAudioVideo {
    pub fn audio() -> Self {
        Self(true)
    }
    pub fn video() -> Self {
        Self(false)
    }

    pub fn is_audio(&self) -> bool {
        self.0
    }
    pub fn is_video(&self) -> bool {
        !self.is_audio()
    }
}

impl std::fmt::Display for IsAudioVideo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_audio() {
            write!(f, "[AUD]")?;
        } else {
            write!(f, "[VID]")?;
        }
        Ok(())
    }
}
