use core::time::Duration;
use reqwest::header::ACCEPT;
use reqwest::header::CONTENT_TYPE;

use futures::future::FutureExt;
use futures::select;
use std::cell::RefCell;
use std::rc::Rc;

use tokio::fs::File;
use tokio::io::AsyncWrite;
use tokio::io::{self, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::Sender;

const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/88.0.4324.190 Safari/537.36";
const CPN_ALPHABET: &[u8] =
    "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_".as_bytes();

#[allow(non_snake_case)]
mod model {
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize, Debug, Clone)]
    pub struct AdaptiveFormat {
        pub itag: i64,
        pub url: String,
        pub bitrate: i64,
        pub mimeType: String,
        //pub contentLength: String,
    }

    #[derive(Serialize, Deserialize, Debug)]
    pub struct StreamingData {
        pub adaptiveFormats: Vec<AdaptiveFormat>,
    }

    #[derive(Serialize, Deserialize, Debug)]
    pub struct VideoDetails {
        pub title: String,
        pub videoId: String,
    }

    #[derive(Serialize, Deserialize, Debug)]
    pub struct PlayerResponse {
        pub videoDetails: VideoDetails,
        pub streamingData: StreamingData,
    }
    // ytInitialPlayerResponse
    // #[derive(Serialize, Deserialize, Debug)]
    // pub struct InitialPlayerResponse {
    //     pub responseContext: PlayerResponse,
    // }

    impl std::fmt::Display for AdaptiveFormat {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{}: {} / {}", self.itag, self.bitrate, self.mimeType)
        }
    }
}

macro_rules! stderr {
    () => (io::stderr().write_all(&[0; 0]).await);
    ($($arg:tt)*) => ({
        io::stderr().write_all(&std::format!($($arg)*).as_bytes()).await
    })
}

fn gen_cpn() -> String {
    use rand::RngCore;
    let mut rng = rand::thread_rng();

    let v: Vec<u8> = (0..16)
        .map(|_| rng.next_u32() & 63)
        .map(|i| CPN_ALPHABET[i as usize])
        .collect();
    String::from_utf8(v).unwrap()
}

async fn download_joiner(
    client: reqwest::Client,
    txbufs: &mut Vec<OutputStreamSender>,
    format: model::AdaptiveFormat,
    consts: Rc<SessionConsts>,
    prefix: &'static str,
    copy_ended: Rc<RefCell<bool>>,
) -> Result<(), anyhow::Error> {
    let (segments_tx, mut segments_rx) = mpsc::channel::<Rc<Segment>>(200);

    stderr!(
        "{}Downloading: {} - {} - {} bit/s, {}\n",
        prefix,
        consts.player.videoDetails.title,
        consts.video_id,
        format.bitrate,
        format.mimeType
    )?;

    start_unordered_download_format(client, format, consts.clone(), segments_tx, prefix);

    while !*copy_ended.borrow() {
        let one_segment_future = join_one_segment(&mut segments_rx, txbufs, prefix.to_string());

        match tokio::time::timeout(Duration::from_millis(12000), one_segment_future).await {
            Err(_) => {
                stderr!("\nordered_download: segment timed out\n")?;
            }
            Ok(_) => {
                // we got the whole segment
            }
        }
    }
    Ok(())
}

// copies from Segment-s in segments_rx
// to channels of Vec<u8> in txbufs
async fn join_one_segment(
    segments_rx: &mut Receiver<Rc<Segment>>,
    txbufs: &mut Vec<OutputStreamSender>,
    prefix: String,
) -> Result<(), anyhow::Error> {
    let res = segments_rx.recv().await;
    match res {
        None => {
            stderr!("\nordered_download: no more segments\n")?;
        }
        Some(segment) => {
            stderr!("{}joining segment {}\n", prefix, segment.sq,)?;
            loop {
                let rx = &segment.rx;
                let mut rx = rx.borrow_mut();

                let out_segment_bytes = rx.recv().await;

                match out_segment_bytes {
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
                                    .map_err(|_| YoutubelinkError::SegmentJoinSendError)?;
                            } else {
                                // non-blocking
                                let _r = out.tx.try_send(bytes.clone());
                            }
                        }
                    }
                    None => {
                        // Sender channel dropped
                        break;
                    }
                }
            }
        }
    }

    Ok(())
}

#[derive(Clone)]
struct AudioVideo {
    audio: Option<model::AdaptiveFormat>,
    video: Option<model::AdaptiveFormat>,
}

fn get_best(formats: &Vec<model::AdaptiveFormat>) -> AudioVideo {
    let mut audio: Option<&model::AdaptiveFormat> = None;
    let mut video: Option<&model::AdaptiveFormat> = None;
    for format in formats {
        if format.mimeType.starts_with("audio/") {
            match audio {
                None => {
                    audio = Some(format);
                }
                Some(old) => {
                    if old.bitrate < format.bitrate {
                        audio = Some(format);
                    }
                }
            }
        } else if format.mimeType.starts_with("video/") {
            match video {
                None => {
                    video = Some(format);
                }
                Some(old) => {
                    if old.bitrate < format.bitrate {
                        video = Some(format);
                    }
                }
            }
        }
    }
    AudioVideo {
        audio: audio.map(|v| v.clone()),
        video: video.map(|v| v.clone()),
    }
}

#[derive(Debug, derive_more::Display)]
pub enum YoutubelinkError {
    #[display(fmt = "VideoParamNotProvided")]
    VideoParamNotProvided,
    #[display(fmt = "player_response not found")]
    PlayerResponseNotFound,

    #[display(fmt = "player_response: parse string: {}", _0)]
    PlayerResponseParseString(serde_json::Error),
    #[display(fmt = "player_response: parse object: {}", _0)]
    PlayerResponseParseObject(serde_json::Error),

    #[display(fmt = "SegmentDataSendError")]
    SegmentDataSendError,

    #[display(fmt = "SegmentSendError")]
    SegmentSendError,

    #[display(fmt = "SegmentJoinSendError")]
    SegmentJoinSendError,

    #[display(fmt = "NoContentTypeHeader")]
    NoContentTypeHeader,

    #[display(fmt = "TCPConnectError: {}", _0)]
    TCPConnectError(std::io::Error),

    #[display(fmt = "UnixConnectError: {}", _0)]
    UnixConnectError(std::io::Error),

    #[display(fmt = "FileCreateError: {}", _0)]
    FileCreateError(std::io::Error),

    #[display(fmt = "NoUnixSocketError: target_os != linux")]
    NoUnixSocketError,
}
impl std::error::Error for YoutubelinkError {}

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    use clap::{App, Arg};
    let matches = App::new("youtubelink")
        .version("0.1.0")
        .author("Szperak")
        .about("youtube-dl")
        .arg(
            Arg::with_name("vout")
                .short("vo")
                .long("vout")
                .takes_value(true)
                .multiple(true)
                .min_values(1)
                .help("Video output: 'stdout' or 'video.mp4' or 'tcp:127.0.0.1:2000' or 'unix:/tmp/video.sock'"),
        )
        .arg(
            Arg::with_name("aout")
                .short("ao")
                .long("aout")
                .takes_value(true)
                .multiple(true)
                .min_values(1)
                .help("Audio output: 'stdout' or 'audio.mp4' or 'tcp:127.0.0.1:2001' or 'unix:/tmp/audio.sock'"),
        )
        .arg(
            Arg::with_name("url")
                .short("u")
                .long("url")
                .takes_value(true)
                .required(true)
                .help("Youtube watch?v= url"),
        )
        .arg(
            Arg::with_name("follow")
                .short("f")
                .long("follow")
                .required(false)
        .takes_value(false)
                .help("True to follow"),
        )
        .get_matches();

    let def = vec!["video.mp4"];
    let mut vout_names: Vec<&str> = matches
        .values_of("vout")
        .map(|v| v.collect::<Vec<_>>())
        .unwrap_or(def);

    let def = vec!["audio.mp4"];
    let mut aout_names: Vec<&str> = matches
        .values_of("aout")
        .map(|v| v.collect::<Vec<_>>())
        .unwrap_or(def);

    let video_link = matches
        .value_of("url")
        .ok_or(YoutubelinkError::VideoParamNotProvided)?;

    let follow_seqnum: bool = matches.is_present("follow");

    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .build()
        .expect("should be able to build reqwest client");

    let mut video_url = video_link.to_string();

    let url = reqwest::Url::parse(&video_url)?;
    for pair in url.query_pairs() {
        if pair.0 == "v" {
            video_url = pair.1.to_string();
        }
    }
    stderr!("video_url: {}\n", video_url)?;
    let player_response = fetch_player_response(&client, video_url.clone()).await?;
    //stderr!("player_response: {:?}\n", player_response)?;

    let formats = &player_response.streamingData.adaptiveFormats;
    for format in formats {
        stderr!("{}\n", format)?;
    }
    let best = get_best(formats);
    stderr!("best audio: {}\n", best.audio.as_ref().unwrap())?;
    stderr!("best video: {}\n", best.video.as_ref().unwrap())?;

    tokio::time::sleep(Duration::from_millis(200)).await;

    let consts = Rc::new(SessionConsts {
        video_id: video_url,
        cpn: gen_cpn(),
        follow_seqnum,
        player: player_response,
    });

    let local = tokio::task::LocalSet::new();
    // Run the local task set.
    local
        .run_until(async move {
            let copy_ended = Rc::new(RefCell::new(false));

            let f1video = start_ordered_download(
                client.clone(),
                &mut vout_names,
                copy_ended.clone(),
                best.video,
                consts.clone(),
                "[VID] ",
            );
            let f2audio = start_ordered_download(
                client.clone(),
                &mut aout_names,
                copy_ended.clone(),
                best.audio,
                consts,
                "[AUD] ",
            );

            let mut f1video = Box::pin(f1video.fuse());
            let mut f2audio = Box::pin(f2audio.fuse());
            let result = select! {
                x = f1video => x,
                x = f2audio => x,
            };
            result?;

            let res: Result<(), anyhow::Error> = Ok(());
            res
        })
        .await?;

    Ok(())
}

async fn start_ordered_download(
    client: reqwest::Client,
    vout_names: &mut Vec<&str>,
    copy_ended: Rc<RefCell<bool>>,
    format: Option<model::AdaptiveFormat>,
    consts: Rc<SessionConsts>,
    prefix: &'static str,
) -> Result<(), anyhow::Error> {
    let vouts = make_outs(vout_names, copy_ended.clone()).await?;
    let mut txbufs = make_out_writers(vouts, copy_ended.clone());

    match format {
        Some(format) => {
            tokio::task::spawn_local(async move {
                download_joiner(client, &mut txbufs, format, consts, prefix, copy_ended)
                    .await
                    .unwrap();
            })
            .await?;
        }
        None => {}
    };
    Ok(())
}

enum SegmentBytes {
    EOF,
    More(Rc<Vec<u8>>),
}

struct Segment {
    sq: i64,
    rx: RefCell<Receiver<SegmentBytes>>,
    tx: Sender<SegmentBytes>,
    //done_tx: Sender<bool>,
}

async fn print_resp_headers(res: &reqwest::Response, prefix: &str) -> Result<(), anyhow::Error> {
    for (key, value) in res.headers() {
        stderr!("{}header: {}: {:?}\n", prefix, key, value)?;
    }
    Ok(())
}

async fn download_format_segment(
    client: reqwest::Client,
    segment: Rc<Segment>,
    format: model::AdaptiveFormat,
    prefix: &str,
    consts: Rc<SessionConsts>,
) -> Result<Option<i64>, anyhow::Error> {
    let tx = segment.tx.clone();
    let mut head_seqnum: Option<i64> = None;

    for _i in 0..3 as usize {
        let segment_url = format!("{}", format.url);

        let sq = segment.sq.to_string();
        let params = [
            ("alr", "yes"),
            ("cpn", &consts.cpn),
            ("cver", "2.20210304.08.01"),
            ("sq", &sq),
            ("rn", "100000000"),
            ("rbuf", "0"),
        ];
        let url = reqwest::Url::parse_with_params(&segment_url, &params)?;
        let req = create_request(url);

        let mut res = client.execute(req).await?;
        stderr!(
            "{}download_format_segment {}: Status: {}\n",
            prefix,
            segment.sq,
            res.status()
        )?;

        let code = res.status().as_u16();
        if code == 200 {
            // OK
            let content_type = res
                .headers()
                .get(CONTENT_TYPE)
                .ok_or(YoutubelinkError::NoContentTypeHeader)?;

            if content_type == "text/plain" {
                stderr!("{}blocked! text/plain\n", prefix)?;
                break;
            }
        } else if code == 204 {
            // No Content
            tokio::time::sleep(Duration::from_millis(50)).await;
            continue;
        } else {
            // 404?
            stderr!(
                "{}failed download sq={}: {}\n",
                prefix,
                segment.sq,
                res.status()
            )?;

            tokio::time::sleep(Duration::from_millis(1000)).await;
            continue;
        }

        res.headers().get("x-head-seqnum").map(|v| {
            //println!("found headnum");
            v.to_str()
                .map(|s| s.parse().map(|parsed| head_seqnum = Some(parsed)))
        });
        //x-head-seqnum

        //print_resp_headers(&res, prefix).await?;

        while let Some(chunk) = res.chunk().await? {
            //use tokio::prelude::*;
            let vec_rc = Rc::new((&chunk).to_vec());

            tx.send(SegmentBytes::More(vec_rc.clone()))
                .await
                .map_err(|_| YoutubelinkError::SegmentDataSendError)?;
            //stderr!(".")?;
            //stderr!("{}{} bytes\n", prefix, chunk.len())?;
        }
        break;
    }
    tx.send(SegmentBytes::EOF)
        .await
        .map_err(|_| YoutubelinkError::SegmentDataSendError)?;

    Ok(head_seqnum)
}

struct SessionConsts {
    cpn: String,
    video_id: String,
    follow_seqnum: bool,
    player: model::PlayerResponse,
}

async fn download_format(
    client: reqwest::Client,
    format: model::AdaptiveFormat,
    prefix: &'static str,
    consts: Rc<SessionConsts>,
    segments_tx: Sender<Rc<Segment>>,
) -> Result<(), anyhow::Error> {
    let mut seqnum: i64 = 0;
    let head_seqnum: Rc<RefCell<i64>> = Rc::new(RefCell::new(0));

    let max_in_flight = 4;
    let (tx_tickets, mut rx_tickets) = mpsc::channel::<bool>(max_in_flight);

    for _i in 0..max_in_flight {
        drop(tx_tickets.try_send(true));
    }

    loop {
        let _ = rx_tickets.recv().await;

        if consts.follow_seqnum {
            let head: i64 = *head_seqnum.borrow();
            if (head - seqnum).abs() > 10 {
                seqnum = head;
                stderr!("{}set seqnum to head: {}\n", prefix, head)?;
            }
        }

        let (tx, rx) = mpsc::channel::<SegmentBytes>(128);
        let segment = Rc::new(Segment {
            sq: seqnum,
            tx: tx,
            rx: RefCell::new(rx),
        });

        segments_tx
            .send(segment.clone())
            .await
            .map_err(|_| YoutubelinkError::SegmentSendError)?;

        {
            let client_clone = client.clone();
            let format = format.clone();
            let consts = consts.clone();
            let tx_tickets = tx_tickets.clone();
            let head_seqnum = head_seqnum.clone();
            tokio::task::spawn_local(async move {
                let res =
                    download_format_segment(client_clone, segment.clone(), format, prefix, consts)
                        .await;
                match res {
                    Err(err) => {
                        let _ = stderr!("{}fetch_segment err: {}\n", prefix, err);
                    }
                    Ok(Some(new_head_seqnum)) => {
                        //let _ = stderr!("{}new_head_seqnum: {}\n", prefix, new_head_seqnum);
                        *(head_seqnum.borrow_mut()) = new_head_seqnum;
                    }
                    _ => {}
                }
                let _ = tx_tickets.send(true).await;
            });
        }

        tokio::time::sleep(Duration::from_millis(20)).await;

        seqnum += 1;
    }
}

fn start_unordered_download_format(
    client: reqwest::Client,
    format: model::AdaptiveFormat,
    consts: Rc<SessionConsts>,
    segments_tx: Sender<Rc<Segment>>,
    prefix: &'static str,
) {
    tokio::task::spawn_local(async move {
        let res = download_format(client, format, prefix, consts, segments_tx).await;
        match res {
            Err(err) => {
                let _ = stderr!("{}download_format err: {}\n", prefix, err);
            }
            _ => {}
        }
    });
}

fn from_slice_lenient<'a, T: serde::Deserialize<'a>>(v: &'a [u8]) -> Result<T, serde_json::Error> {
    let mut cur = std::io::Cursor::new(v);
    let mut de = serde_json::Deserializer::new(serde_json::de::IoRead::new(&mut cur));
    serde::Deserialize::deserialize(&mut de)
}

fn create_request(url: reqwest::Url) -> reqwest::Request {
    let mut req = reqwest::Request::new(reqwest::Method::GET, url);
    let h = req.headers_mut();
    h.insert("accept", "*/*".parse().unwrap());
    h.insert(
        "accept-language",
        "pl,en-US;q=0.9,en;q=0.8,pl-PL;q=0.7".parse().unwrap(),
    );
    h.insert("cache-control", "no-cache".parse().unwrap());
    h.insert("origin", "https://www.youtube.com".parse().unwrap());
    h.insert("pragma", "no-cache".parse().unwrap());
    h.insert("referer", "https://www.youtube.com/".parse().unwrap());
    h.insert("sec-fetch-dest", "empty".parse().unwrap());
    h.insert("sec-fetch-mode", "cors".parse().unwrap());
    h.insert("sec-fetch-site", "cross-site".parse().unwrap());
    h.insert("user-agent", USER_AGENT.parse().unwrap());
    //h.insert("x-client-data", "xxxxxxxxxx==".parse().unwrap());
    req
}

async fn fetch_player_response(
    client: &reqwest::Client,
    video_base64_name: String,
) -> Result<model::PlayerResponse, anyhow::Error> {
    let watch_url = format!("https://youtube.com/watch");

    let params = [("v", video_base64_name)];
    let url = reqwest::Url::parse_with_params(&watch_url, &params)?;
    let mut req = reqwest::Request::new(reqwest::Method::GET, url);
    let h = req.headers_mut();
    h.insert(ACCEPT, "*/*".parse().unwrap());
    h.insert(
        "accept-language",
        "pl,en-US;q=0.9,en;q=0.8,pl-PL;q=0.7".parse().unwrap(),
    );
    h.insert("cache-control", "no-cache".parse().unwrap());
    h.insert("pragma", "no-cache".parse().unwrap());
    h.insert("sec-fetch-dest", "empty".parse().unwrap());
    h.insert("sec-fetch-mode", "cors".parse().unwrap());
    h.insert("sec-fetch-site", "cross-site".parse().unwrap());
    h.insert("user-agent", USER_AGENT.parse().unwrap());
    //h.insert("x-client-data", "xxxxxxxxxx==".parse().unwrap());

    let res = client.execute(req).await?;
    stderr!("fetch_player_response: Status: {}\n", res.status())?;

    let bytes_vec = res.bytes().await?;
    // stderr!(
    //     "fetch_player_response: {}\n",
    //     String::from_utf8_lossy(&bytes_vec)
    // )?;

    const PLAYER_RESPONSE_PATTERN: &'static [u8] = r#"var ytInitialPlayerResponse = "#.as_bytes();

    let found_index = twoway::find_bytes(&bytes_vec, PLAYER_RESPONSE_PATTERN);
    match found_index {
        Some(player_index) => {
            let l = PLAYER_RESPONSE_PATTERN.len() - 1;
            let player_resp_bytes = &bytes_vec[player_index + l..];
            // stderr!(
            //     "fetch_player_response: {}\n",
            //     String::from_utf8_lossy(&player_resp_bytes)
            // )?;

            // let player_response: String = from_slice_lenient(&player_resp_bytes)
            //     .map_err(|e| YoutubelinkError::PlayerResponseParseString(e))?;

            let player_response: model::PlayerResponse = from_slice_lenient(&player_resp_bytes)
                .map_err(|e| YoutubelinkError::PlayerResponseParseObject(e))?;

            // if true {
            //     let mut file = File::create("debug/player_response.json")
            //         .await
            //         .map_err(|err| YoutubelinkError::FileCreateError(err))?;
            //     file.write_all(player_response.as_bytes()).await?;
            // }

            Ok(player_response)
        }
        None => Err(YoutubelinkError::PlayerResponseNotFound.into()),
    }
}

struct OutputStreamSender {
    reliable: bool,
    tx: Sender<Rc<Vec<u8>>>,
}

fn make_out_writers(
    outs: Vec<AsyncOutput>,
    copy_ended: Rc<RefCell<bool>>,
) -> Vec<OutputStreamSender> {
    let mut txbufs = Vec::new();
    for mut out in outs {
        let channel_size = 256;
        let (txbuf, rxbuf) = mpsc::channel::<Rc<Vec<u8>>>(channel_size);
        let copy_endedc = copy_ended.clone();

        txbufs.push(OutputStreamSender {
            tx: txbuf,
            reliable: out.reliable,
        });
        tokio::task::spawn_local(async move {
            let res = copy_channel_to_out(rxbuf, &mut out).await;
            match res {
                Err(err) => {
                    let _ = stderr!("copy_channel_to_out err: {}\n", err);
                }
                Ok(_) => {}
            }
            let _ = stderr!("copy_channel_to_out ending\n");
            *copy_endedc.borrow_mut() = true;
        });
    }
    txbufs
}

struct AsyncOutput {
    writer: Box<dyn AsyncWrite + Unpin>,
    reliable: bool,
}

impl AsyncOutput {
    fn new(writer: Box<dyn AsyncWrite + Unpin>) -> Self {
        Self {
            writer,
            reliable: true,
        }
    }
    fn new_unreliable(writer: Box<dyn AsyncWrite + Unpin>) -> Self {
        Self {
            writer,
            reliable: false,
        }
    }
}

async fn make_outs(
    out_names: &mut Vec<&str>,
    copy_ended: Rc<RefCell<bool>>,
) -> Result<Vec<AsyncOutput>, anyhow::Error> {
    let mut outs: Vec<AsyncOutput> = Vec::new();

    while let Some(out_name) = out_names.pop() {
        match out_name {
            "out" => {
                outs.push(AsyncOutput::new(Box::new(io::stdout())));
            }
            "stdout" => {
                outs.push(AsyncOutput::new(Box::new(io::stdout())));
            }
            "stderr" => {
                outs.push(AsyncOutput::new(Box::new(io::stderr())));
            }
            "ffplay" => {
                out_names.push("ffvideo");
                out_names.push("ffaudio");
            }
            "ffvideo" => {
                let stdin_video = spawn_ffplay(&copy_ended, "youtubezero", false);
                outs.push(AsyncOutput::new(Box::new(stdin_video)));
            }
            "ffaudio" => {
                let stdin_audio = spawn_ffplay(&copy_ended, "youtubezero", true);
                outs.push(AsyncOutput::new(Box::new(stdin_audio)));
            }
            other_out => {
                const TCP_O: &str = "tcp:";
                const UNIX_O: &str = "unix:";
                if other_out.starts_with(TCP_O) {
                    let addr = &other_out[TCP_O.len()..];
                    let addr = addr.strip_prefix("//").unwrap_or(addr);
                    use tokio::net::TcpStream;
                    let stream = TcpStream::connect(addr)
                        .await
                        .map_err(|err| YoutubelinkError::TCPConnectError(err))?;
                    outs.push(AsyncOutput::new(Box::new(stream)));
                } else if other_out.starts_with(UNIX_O) {
                    #[cfg(target_os = "linux")]
                    {
                        let addr = &other_out[UNIX_O.len()..];
                        let addr = addr.strip_prefix("//").unwrap_or(addr);
                        use tokio::net::UnixStream;

                        let stream = UnixStream::connect(addr)
                            .await
                            .map_err(|err| YoutubelinkError::UnixConnectError(err))?;
                        outs.push(AsyncOutput::new(Box::new(stream)));
                    }
                    #[cfg(not(target_os = "linux"))]
                    {
                        return Err(YoutubelinkError::NoUnixSocketError.into());
                    }
                } else {
                    let file = File::create(other_out)
                        .await
                        .map_err(|err| YoutubelinkError::FileCreateError(err))?;
                    outs.push(AsyncOutput::new(Box::new(file)));
                }
            }
        }
    }
    Ok(outs)
}

async fn copy_channel_to_out(
    mut rxbuf: Receiver<Rc<Vec<u8>>>,
    out: &mut AsyncOutput,
) -> Result<(), anyhow::Error> {
    loop {
        // Give some time to other tasks
        tokio::task::yield_now().await;

        // We'll receive from channel
        // in the future
        let f = rxbuf.recv();

        let once_msg: Option<Option<Rc<Vec<u8>>>> = f.now_or_never();
        let msg: Option<Rc<Vec<u8>>>;
        match once_msg {
            // Received instantly
            Some(m) => {
                msg = m;
            }
            None => {
                out.writer.flush().await?;
                //stderr!("flushed")?;

                msg = rxbuf.recv().await;
            }
        }

        match msg {
            Some(bytes_vec) => {
                //stderr!("+")?;
                out.writer.write_all(&bytes_vec).await?;
            }
            // None from .recv()
            // because channel is closed
            None => {
                break;
            }
        }
    }
    Ok(())
}

fn spawn_ffplay(
    copy_ended: &Rc<RefCell<bool>>,
    channel: &str,
    onlyaudio: bool,
) -> tokio::process::ChildStdin {
    use std::process::Stdio;
    use tokio::process::Command;

    let mut cmd = Box::new(Command::new("ffplay"));

    // audio
    // ffplay -window_title "%channel%" -fflags nobuffer -flags low_delay -af volume=0.2 -x 1280 -y 720 -i -

    // video
    // ffplay -window_title "%channel%" -probesize 32 -sync ext -af volume=0.3 -vf setpts=0.5*PTS -x 1280 -y 720 -i -

    if onlyaudio {
        cmd.arg("-window_title").arg(format!("{} audio", channel));
        cmd.arg("-fflags")
            .arg("nobuffer")
            .arg("-flags")
            .arg("low_delay")
            .arg("-vn")
            .arg("-framedrop")
            .arg("-af")
            .arg("volume=0.4,atempo=1.005")
            .arg("-strict")
            .arg("experimental")
            .arg("-x")
            .arg("1280")
            .arg("-y")
            .arg("720")
            .arg("-i")
            .arg("-");
    } else {
        cmd.arg("-window_title")
            .arg(format!("{} - lowest latency", channel));

        cmd.arg("-probesize")
            .arg("32")
            .arg("-sync")
            .arg("ext")
            .arg("-framedrop")
            .arg("-vf")
            .arg("setpts=0.5*PTS")
            .arg("-x")
            .arg("1280")
            .arg("-y")
            .arg("720")
            .arg("-i")
            .arg("-");
    }

    cmd.stderr(Stdio::null()).stdin(Stdio::piped());

    let mut child = cmd.spawn().expect("failed to spawn ffplay");

    let stdin = child
        .stdin
        .take()
        .expect("child did not have a handle to stdin");

    let process_ended = copy_ended.clone();
    tokio::task::spawn_local(async move {
        let status = child
            .wait()
            .await
            .expect("child process encountered an error");

        let _ = stderr!("child status was: {}", status);
        *process_ended.borrow_mut() = true;
    });

    stdin
}
