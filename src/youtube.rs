//use macros::stderr;
use crate::segmenter::NextStep;
use crate::segmenter::OrderingEvent;
use crate::segmenter::Segment;
use crate::ytzero::IsAudioVideo;
use crate::ytzero::SessionConsts;
use crate::zeroerror::Result;
use crate::zeroerror::ResultExt;
use crate::zeroerror::YoutubezeroError;
use bytes::Bytes;
use core::ops::RangeInclusive;
use core::time::Duration;
use error_chain::ChainedError;
use memchr::memmem::find_iter as mem_find_iter;
use rand::thread_rng;
use rand::Rng;
use reqwest::header::HeaderValue;
use reqwest::header::CONTENT_TYPE;
use std::cell::RefCell;
use std::convert::TryFrom;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc::Sender;

use reqwest::header::ACCEPT;

use crate::model;
use crate::stderr;
use crate::stderr_result;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;

pub const MY_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/101.0.4951.67 Safari/537.36";
pub const CPN_ALPHABET: &[u8] =
    "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_".as_bytes();

pub fn gen_cpn() -> String {
    use rand::RngCore;
    let mut rng = rand::thread_rng();

    let v: Vec<u8> = (0..16)
        .map(|_| rng.next_u32() & 63)
        .map(|i| CPN_ALPHABET[i as usize])
        .collect();
    String::from_utf8_lossy(&v).into()
}

pub fn create_request(url: reqwest::Url) -> reqwest::Request {
    let mut req = reqwest::Request::new(reqwest::Method::GET, url);
    let h = req.headers_mut();
    h.insert("accept", HeaderValue::from_static("*/*"));
    h.insert(
        "accept-language",
        HeaderValue::from_static("en-US,en;q=0.9"),
    );
    //h.insert("cache-control", "no-cache"));
    h.insert(
        "origin",
        HeaderValue::from_static("https://www.youtube.com"),
    );
    //h.insert("pragma", "no-cache"));
    h.insert(
        "referer",
        HeaderValue::from_static("https://www.youtube.com/"),
    );
    h.insert("sec-fetch-dest", HeaderValue::from_static("empty"));
    h.insert("sec-fetch-mode", HeaderValue::from_static("cors"));
    h.insert("sec-fetch-site", HeaderValue::from_static("cross-site"));
    h.insert("user-agent", HeaderValue::from_static(MY_USER_AGENT));
    h.insert("x-client-data", HeaderValue::from_static("CI3yygE="));
    req
}

pub async fn fetch_watch_v(client: &reqwest::Client, video_base64_name: &str) -> Result<Bytes> {
    let watch_url = format!("https://youtube.com/watch");

    let params = [("v", video_base64_name)];
    let url = reqwest::Url::parse_with_params(&watch_url, &params)?;
    let mut req = reqwest::Request::new(reqwest::Method::GET, url);
    let h = req.headers_mut();
    h.insert(ACCEPT, HeaderValue::from_static("*/*"));
    h.insert(
        "accept-language",
        HeaderValue::from_static("en-US,en;q=0.9"),
    );
    //h.insert("pragma", "no-cache"));
    //h.insert("cache-control", "no-cache"));
    h.insert(
        "sec-ch-ua",
        HeaderValue::from_static(r#"" Not A;Brand";v="99", "Chromium";v="102""#),
    );
    h.insert(
        "origin",
        HeaderValue::from_static("https://www.youtube.com"),
    );
    h.insert(
        "referer",
        format!("https://www.youtube.com/watch?v={}", video_base64_name).parse()?,
    );
    h.insert("sec-ch-ua-mobile", HeaderValue::from_static("?0"));
    h.insert("sec-fetch-dest", HeaderValue::from_static("empty"));
    h.insert("sec-fetch-mode", HeaderValue::from_static("navigate"));
    h.insert("sec-fetch-site", HeaderValue::from_static("cross-site"));
    h.insert("user-agent", HeaderValue::from_static(MY_USER_AGENT));
    h.insert("x-client-data", HeaderValue::from_static("CI3yygE="));

    let res = client.execute(req).await?;
    stderr!("fetch_player_response: Status: {}\n", res.status());

    let bytes_vec = res.bytes().await?;

    Ok(bytes_vec)
}

#[derive(Clone, Debug)]
pub enum PlayerResponseSource {
    VideoIdBase64(String),
    LocalWatchvIndexPath(String),
}

impl TryFrom<&str> for PlayerResponseSource {
    type Error = url::ParseError;
    fn try_from(url_or_path: &str) -> std::result::Result<Self, Self::Error> {
        if let Some(local_path) = url_or_path.strip_prefix("file://") {
            Ok(PlayerResponseSource::LocalWatchvIndexPath(
                local_path.to_string(),
            ))
        } else {
            let mut video_id: String = url_or_path.to_string();
            let url = reqwest::Url::parse(&video_id)?;
            let pairs = url.query_pairs();
            for pair in pairs {
                if pair.0 == "v" {
                    video_id = pair.1.to_string();
                }
            }
            Ok(PlayerResponseSource::VideoIdBase64(video_id))
        }
    }
}

pub async fn fetch_player_response(
    client: &reqwest::Client,
    source: &PlayerResponseSource,
) -> Result<(model::PlayerResponse, Option<String>)> {
    let bytes_vec: Bytes = match source {
        PlayerResponseSource::VideoIdBase64(video_id) => {
            fetch_watch_v(client, video_id.as_str()).await?
        }
        PlayerResponseSource::LocalWatchvIndexPath(path_str) => {
            let pathbuf = PathBuf::from(path_str);
            let wbytes = read_file_bytes(&pathbuf).await?;
            wbytes.ok_or("read_file_bytes: local file does not exist")?
        }
    };

    let base_js_url = crate::extras::nfuncextract::extract_base_js_url(&bytes_vec);

    //let bytes_vec = include_bytes!("../debug/watchvreplace.html").to_vec();
    let write_debug_files = true;
    if write_debug_files {
        tokio::fs::create_dir_all(&"debug")
            .await
            .chain_err(|| format!("create_dir_all {:?}", "debug"))?;
        let debugname = "debug/watchvindex.html";
        let mut file = File::create(debugname)
            .await
            .chain_err(|| format!("create debug file {}", debugname))?;
        file.write_all(&bytes_vec).await?;
    }
    // stderr!(
    //     "fetch_player_response: {}\n",
    //     String::from_utf8_lossy(&bytes_vec)
    // )?;

    const PATTERN_START: &'static [u8] = r#"var ytInitialPlayerResponse = "#.as_bytes();

    // optional
    const PATTERN_END_HINT: &'static [u8] =
        r#"var meta = document.createElement('meta');"#.as_bytes();

    let found_start = mem_find_iter(&bytes_vec, PATTERN_START).next();
    if let Some(found_start) = found_start {
        let l = PATTERN_START.len() - 1;
        let mut player_resp_bytes = &bytes_vec[found_start + l..];

        let found_end = mem_find_iter(&player_resp_bytes, PATTERN_END_HINT).next();
        if let Some(found_end) = found_end {
            player_resp_bytes = &player_resp_bytes[..found_end];
        }
        // stderr!(
        //     "fetch_player_response: {}\n",
        //     String::from_utf8_lossy(&player_resp_bytes)
        // );

        // let player_response: String = from_slice_lenient(&player_resp_bytes)
        //     .map_err(|e| YoutubelinkError::PlayerResponseParseString(e))?;

        if write_debug_files {
            let player_response: serde_json::Value = from_slice_lenient(&player_resp_bytes)?;
            let debugname = "debug/player_response.json";
            let mut file = File::create(debugname)
                .await
                .chain_err(|| format!("create debug file {}", debugname))?;
            file.write_all(&serde_json::to_vec(&player_response)?)
                .await?;
        }

        let player_response: model::PlayerResponse = from_slice_lenient(&player_resp_bytes)
            .chain_err(|| "from_slice_lenient PlayerResponseParseObject")?;

        Ok((player_response, base_js_url))
    } else {
        return Err(YoutubezeroError::PlayerResponseNotFound.into());
    }
}

fn from_slice_lenient<'a, T: serde::Deserialize<'a>>(
    v: &'a [u8],
) -> std::result::Result<T, serde_json::Error> {
    let mut cur = std::io::Cursor::new(v);
    let mut de = serde_json::Deserializer::new(serde_json::de::IoRead::new(&mut cur));
    serde::Deserialize::deserialize(&mut de)
}

pub fn sq_to_range(
    format: &model::AdaptiveFormat,
    consts: &Rc<SessionConsts>,
    segment: &Rc<Segment>,
) -> std::ops::RangeInclusive<i64> {
    let content_len = format.content_length.unwrap_or_default();
    let sq = segment.sq;
    let fake_seg_size: i64 = consts.fake_segment_size;
    let from = sq * fake_seg_size;
    let to = from + fake_seg_size - 1;
    let to = to.min(content_len - 1);
    // inclusive
    let range = from..=to;

    range
    //file_range = Some(range.clone());
}

struct SegmentRequest<'a> {
    pub segment_url: String,
    pub params: Vec<(&'static str, String)>,
    pub is_live_segmented: bool,
    //pub request_number: i64,
    pub range_str: Option<String>,
    pub file_range: Option<RangeInclusive<i64>>,

    segment: &'a Rc<Segment>,
    tx: &'a Sender<OrderingEvent>,
    isav: IsAudioVideo,
    format: &'a Rc<RefCell<model::AdaptiveFormat>>,
    consts: &'a Rc<SessionConsts>,
    head_seqnum_rc: &'a Rc<RefCell<i64>>,

    request_get_start: Instant,
}

fn init_yt_params(sr: &mut SegmentRequest) {
    let rn = sr.consts.next_request_number().to_string();

    sr.params.push(("alr", "yes".to_string()));
    sr.params.push(("cpn", sr.consts.cpn.to_string()));
    sr.params.push(("cver", "2.20220613.00.00".to_string()));
    sr.params.push(("rn", rn));
    sr.params.push(("rbuf", "0".to_string()));

    if sr.segment.requested_head {
        sr.params.push(("headm", "3".to_string()));
    } else if sr.is_live_segmented {
        // is live?
        let sq_str = sr.segment.sq;
        sr.params.push(("sq", sq_str.to_string()));
    } else {
        let range = sq_to_range(&sr.format.borrow(), sr.consts, sr.segment);
        let ranges = format!("{}-{}", range.start(), range.end());
        sr.params.push(("range", ranges.clone()));
        sr.range_str = Some(ranges);
        sr.file_range = Some(range);
    }
}

pub async fn download_format_segment_once(
    client: &reqwest::Client,
    segment: &Rc<Segment>,
    tx: &Sender<OrderingEvent>,
    format: &Rc<RefCell<model::AdaptiveFormat>>,
    isav: IsAudioVideo,
    consts: &Rc<SessionConsts>,
    head_seqnum_rc: &Rc<RefCell<i64>>,
) -> Result<NextStep> {
    let segment_url = format
        .borrow()
        .get_live_or_offlive_url()
        .ok_or("url and non_live_url not provided in AdaptiveFormat")?
        .to_string();
    let mut sr = SegmentRequest {
        segment_url,
        params: Vec::new(),
        is_live_segmented: consts.player.videoDetails.isLive.unwrap_or_default(),

        file_range: None,
        range_str: None,

        request_get_start: Instant::now(),

        segment,
        tx,
        format,
        isav,
        consts,
        head_seqnum_rc,
    };

    init_yt_params(&mut sr);

    let url = parse_with_params_override(&sr.segment_url, &sr.params)?;

    // let q = url.query();
    // let q = format!("{}&pot=GpsBCm6FIMirvHnqaajAY8YYZnTKvv36vihh7yJ_kszAnuxRqmO7r-2ySIIC7mvBXogNi-qGi3108GHTgImZh1bnMU3HOSLaNIyUK5zdewg0yGvs8_Y3hTA0YUqtzCMhiNX5gR41kvgu-fbkDpBd3EKNlRIpATwYQQ6Yl8jnK_-zNk2wf1hFjUcsCFQMM3DpevmSrllN-L9lOIQthmo=", q.unwrap());
    // url.set_query(Some(&q));

    if false {
        let urlstr = url.as_str();
        let urlstr = urlstr.replace("&", "&\n");
        stderr!(
            "{} download_format_segment {}/{}: start GET {}\n",
            isav,
            segment.sq,
            head_seqnum_rc.borrow(),
            urlstr
        );
    } else {
        stderr!(
            "{} download_format_segment {} {}/{}: start GET\n",
            isav,
            sr.range_str.as_ref().map(|s| s.as_str()).unwrap_or(""),
            segment.sq,
            head_seqnum_rc.borrow(),
        );
    }

    let req = create_request(url);
    

    
    // if let Some(range) = file_range.as_ref() {
    //     let h = req.headers_mut();
    //     h.insert(
    //         "Range",
    //         format!("bytes={}-{}", range.start(), range.end())
    //             .parse()
    //             .unwrap(),
    //     );
    // }

    let resp = client.execute(req).await?;

    handle_headers(&sr, &resp);

    stderr!(
        "{} download_format_segment {} {}/{}: Status: {} ({}ms since GET)\n",
        isav,
        sr.range_str.as_ref().map(|s| s.as_str()).unwrap_or(""),
        segment.sq,
        head_seqnum_rc.borrow(),
        resp.status(),
        sr.request_get_start.elapsed().as_millis()
    );

    let next_step = handle_status_codes(&sr, resp).await;
    let resp = match next_step {
        Ok(NextStep::Download(resp)) => resp,
        result => return result,
    };
    let resp_stream_start = std::time::Instant::now();
    //print_resp_headers(&res, prefix).await?;
    let transmitted = download_body(&sr, resp).await?;
    log_stats_done(&sr, transmitted, &resp_stream_start).await?;

    Ok(NextStep::Stop)
}
fn handle_headers(sr: &SegmentRequest, resp: &reqwest::Response) {
    if let Some(parsed_head) = opt_resp_header(&resp, "x-head-seqnum") {
        //println!("found headnum");
        if sr.is_live_segmented {
            *sr.head_seqnum_rc.borrow_mut() = parsed_head;
        }
    }

    if let Some(parsed_sq) = opt_resp_header(&resp, "x-sequence-num") {
        let mut fixed_sq = sr.segment.fixed_sq.borrow_mut();
        if parsed_sq != *fixed_sq {
            if sr.segment.requested_head {
                *fixed_sq = parsed_sq;
            } else {
                // stderr!(
                //     "{} warn: response wants to change seqnum from {} to {}/{}\n",
                //     isav,
                //     *sq,
                //     parsed_sq,
                //     head_seqnum_rc.borrow(),
                // );
            }
        }
    }
}

async fn handle_status_codes(sr: &SegmentRequest<'_>, resp: reqwest::Response) -> Result<NextStep> {
    let code = resp.status().as_u16();
    if code == 200 || code == 206 {
        // OK
        let content_type = resp
            .headers()
            .get(CONTENT_TYPE)
            .ok_or(YoutubezeroError::NoContentTypeHeader)?;

        if content_type == "text/plain" {
            stderr!("{} blocked! text/plain\n", sr.isav);
            let bytes = resp.bytes().await?;
            let bodystr = String::from_utf8_lossy(&bytes);
            return Ok(NextStep::RetryRedirect(bodystr.to_string()));
        }
    } else if code == 204 {
        // No Content
        tokio::time::sleep(Duration::from_millis(100)).await;
        return Ok(NextStep::Retry);
    } else if code == 403 {
        // Forbidden
        let bytes = resp.bytes().await?;
        stderr!(
            "{} Forbidden msg: {:?} // restart the program maybe?\n",
            sr.isav,
            String::from_utf8_lossy(&bytes[..bytes.len().min(30)])
        );
        return Ok(NextStep::Retry);
    } else if code == 503 {
        // Service Unavailable
        return Ok(NextStep::Stop);
    } else {
        // 404?
        stderr!(
            "{} failed download {} sq={}/{} range={:?}: {}\n",
            sr.isav,
            sr.range_str.as_ref().map(|s| s.as_str()).unwrap_or(""),
            sr.segment.sq,
            sr.head_seqnum_rc.borrow(),
            sr.range_str,
            resp.status()
        );
        if code == 404 && sr.segment.sq == 0 && sr.consts.follow_head_seqnum {
            return Ok(NextStep::Stop);
        }
        tokio::time::sleep(Duration::from_millis(1000)).await;
        return Ok(NextStep::Retry);
    }
    Ok(NextStep::Download(resp))
}

async fn log_stats_done(
    sr: &SegmentRequest<'_>,
    transmitted: usize,
    resp_stream_start: &Instant,
) -> Result<()> {
    if let Some(range) = &sr.file_range {
        let requested = range.end() - range.start() + 1;
        if requested != transmitted as i64 {
            stderr!(
                "{} warn: sq={} requested != transmitted, {} != {}\n",
                sr.isav,
                sr.segment.sq,
                requested,
                transmitted,
            );
        }
    }

    let micros_elapsed: f64 = (resp_stream_start.elapsed().as_micros() as f64).abs() + 0.1;
    let bytes_per_second = (transmitted as f64) / (micros_elapsed / 1000000.0);
    let mbytes_per_second = bytes_per_second / 1024.0 / 1024.0;
    stderr!(
        "{} sent <- segment {} sq={} speed: {:.3} MB/s ({:04}ms since OK, {:04}ms since GET)\n",
        sr.isav,
        sr.range_str.as_ref().map(|s| s.as_str()).unwrap_or(""),
        sr.segment.sq,
        mbytes_per_second,
        resp_stream_start.elapsed().as_millis(),
        sr.request_get_start.elapsed().as_millis(),
    );
    Ok(())
}

async fn download_body(sr: &SegmentRequest<'_>, mut resp: reqwest::Response) -> Result<usize> {
    let mut transmitted = 0;
    // Response Headers received so we know 100% the sequence number
    let sq = sr.segment.sq;
    if sr.consts.whole_segments {
        let bytes = resp.bytes().await?;

        let len = bytes.len();
        transmitted += len;
        sr.tx
            .send(OrderingEvent::SegmentData(sq, bytes.clone()))
            .await
            .map_err(|_| YoutubezeroError::SegmentDataSendError)?;
        //stderr!("{} seg={}   {} full bytes\n", prefix, segment.sq, len)?;

        if sr.consts.cache_segments {
            let bytes = bytes.clone();
            let consts = sr.consts.clone();
            
            let isav = sr.isav;
            tokio::task::spawn_local(async move {
                let result = write_segment_file(&bytes, sq, consts, isav).await;
                if let Err(err) = result {
                    let _ = stderr_result!(
                        "{} write_segment_file err: {}\n",
                        isav,
                        err.display_chain().to_string()
                    );
                }
            });
        }
    } else {
        while let Some(chunk) = resp.chunk().await? {
            let len = chunk.len();
            transmitted += len;
            sr.tx
                .send(OrderingEvent::SegmentData(sq, chunk))
                .await
                .map_err(|_| YoutubezeroError::SegmentDataSendError)?;
            //stderr!(".")?;
            //stderr!("{} seg={}   {} bytes\n", isav, *segment.sq.borrow(), len);
        }
    }
    Ok(transmitted)
}

pub fn opt_resp_header<T: std::str::FromStr>(res: &reqwest::Response, key: &str) -> Option<T> {
    res.headers()
        .get(key)
        .map(|v| v.to_str().ok())
        .flatten()
        .map(|s| s.parse().ok())
        .flatten()
}

pub async fn read_file_bytes(file_path: &PathBuf) -> Result<Option<Bytes>> {
    let meta = tokio::fs::metadata(&file_path).await;
    let is_file = meta.as_ref().map(|meta| meta.is_file()).unwrap_or_default();
    if is_file {
        let len: u64 = meta.map(|meta| meta.len()).unwrap_or(1024);
        let mut buf = Vec::with_capacity(len as usize);
        {
            let mut file = File::open(&file_path)
                .await
                .chain_err(|| format!("File::open {:?}", file_path))?;
            file.read_to_end(&mut buf).await?;
        }
        return Ok(Some(buf.into()));
    }
    Ok(None)
}

pub async fn provide_from_cache(
    sq: i64,
    isav: IsAudioVideo,
    consts: &Rc<SessionConsts>,
) -> Result<Option<Bytes>> {
    if let Some(segment_path) = consts.segment_path(sq, isav) {
        if let Some(sbytes) = read_file_bytes(&segment_path).await? {
            return Ok(Some(sbytes));
        }
    }

    Ok(None)
}

pub async fn maybe_use_file_cache(
    segment: &Rc<Segment>,
    tx: &Sender<OrderingEvent>,
    isav: IsAudioVideo,
    consts: &Rc<SessionConsts>,
    head_seqnum_rc: &Rc<RefCell<i64>>,
) -> Result<bool> {
    let head: i64 = *head_seqnum_rc.borrow();
    if consts.cache_segments && head != -1 {
        let sq: i64 = segment.sq;
        let buf = provide_from_cache(sq, isav, consts).await?;
        if let Some(buf) = buf {
            let msgs = [OrderingEvent::SegmentData(sq, buf.into()), OrderingEvent::SegmentEof(sq)];
            for msg in msgs {
                tx.send(msg)
                    .await
                    .map_err(|_| YoutubezeroError::SegmentDataSendError)?;
            }
            return Ok(true);
        }
    }

    Ok(false)
}

pub async fn download_format_segment_retrying(
    client: reqwest::Client,
    segment: Rc<Segment>,
    tx: Sender<OrderingEvent>,
    format: Rc<RefCell<model::AdaptiveFormat>>,
    isav: IsAudioVideo,
    consts: Rc<SessionConsts>,
    head_seqnum_rc: Rc<RefCell<i64>>,
) -> Result<()> {
    if maybe_use_file_cache(&segment, &tx, isav, &consts, &head_seqnum_rc).await? {
        // make sure channel is dropped
        drop(tx);
    } else {
        for _i in 0..consts.retries as usize {
            let once_downloader = download_format_segment_once(
                &client,
                &segment,
                &tx,
                &format,
                isav,
                &consts,
                &head_seqnum_rc,
            );
            match tokio::time::timeout(consts.timeout_one_request, once_downloader).await {
                Err(_) => {
                    stderr!("{} timeout sq={}\n", isav, segment.sq,);
                    continue;
                }
                Ok(Err(err)) => {
                    return Err(err);
                }
                Ok(Ok(NextStep::Retry)) => {
                    continue;
                }
                Ok(Ok(NextStep::Download(_))) => {
                    unreachable!();
                }
                Ok(Ok(NextStep::RetryRedirect(redirect_url))) => {
                    stderr!(
                        "{} redirect sq={} url:{}\n",
                        isav,
                        segment.sq,
                        redirect_url
                    );
                    {
                        let mut f = format.borrow_mut();
                        if f.url.is_some() {
                            f.url = Some(redirect_url);
                        }
                    }
                    continue;
                }
                Ok(Ok(NextStep::Stop)) => {
                    break;
                }
            }
        }
        tx.send(OrderingEvent::SegmentEof(segment.sq))
            .await
            .map_err(|_| YoutubezeroError::SegmentDataSendError)?;
        drop(tx);
    }

    Ok(())
}

async fn write_segment_file(
    segment_bytes: &Bytes,
    sq: i64,
    consts: Rc<SessionConsts>,
    isav: IsAudioVideo,
) -> Result<()> {
    if let Some(file_path_save) = consts.segment_path(sq, isav) {
        let dir = {
            let mut p = file_path_save.clone();
            p.pop();
            p
        };
        tokio::fs::create_dir_all(&dir)
            .await
            .chain_err(|| format!("create_dir_all {:?}", dir))?;
        let file_path_temp = {
            let mut p = dir.clone();
            let randn: u32 = thread_rng().gen();
            p.push(format!("seg{}rand{}.temp", sq, randn));
            p
        };

        {
            let mut file = File::create(&file_path_temp)
                .await
                .chain_err(|| format!("File::create {:?}", file_path_temp))?;
            file.write_all(segment_bytes).await?;
        }
        tokio::fs::rename(&file_path_temp, &file_path_save)
            .await
            .chain_err(|| format!("tokio::fs::rename to {:?}", file_path_save))?;
    }
    Ok(())
}

// pub fn replace_or_append_pairs(url: &mut reqwest::Url, new_pairs: &[(&str, &str)]) {
//     let keys: Vec<String> = url.query_pairs().map(|v| v.0.into()).collect();
//     for new_pair in new_pairs {
//         let mut keysit = keys.iter().map(|v| v.as_str());
//         if !keysit.any(|v| v == new_pair.0) {
//             url.query_pairs_mut().append_pair(new_pair.0, new_pair.1);
//         }
//     }
// }

#[allow(dead_code)]
pub async fn print_resp_headers(res: &reqwest::Response, prefix: &str) -> Result<()> {
    for (key, value) in res.headers() {
        stderr!("{}header: {}: {:?}\n", prefix, key, value);
    }
    Ok(())
}

pub fn parse_with_params_override<I, K, V>(
    input: &str,
    params_new: I,
) -> std::result::Result<url::Url, url::ParseError>
where
    I: IntoIterator,
    I::Item: std::borrow::Borrow<(K, V)>,
    K: AsRef<str>,
    V: AsRef<str>,
{
    let mut url = reqwest::Url::options().parse(input);
    use std::borrow::Borrow;
    if let Ok(ref mut url) = url {
        let params_old: Vec<_> = url
            .query_pairs()
            .map(|pair| (pair.0.to_string(), pair.1.to_string()))
            .collect();
        let mut params_new: Vec<Option<(String, String)>> = params_new
            .into_iter()
            .map(|v| {
                Some((
                    v.borrow().0.as_ref().to_string(),
                    v.borrow().1.as_ref().to_string(),
                ))
            })
            .collect();

        url.query_pairs_mut()
            .clear()
            .extend_pairs(params_old.iter().map(|pair_old| {
                if let Some(pair_new) = params_new.iter_mut().find_map(|opt_pair_new| {
                    if let Some(pair_new) = opt_pair_new {
                        if pair_new.0 == pair_old.0 {
                            Some(opt_pair_new.take().expect("Some"))
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                }) {
                    pair_new
                } else {
                    pair_old.clone()
                }
            }))
            .extend_pairs(params_new.iter().filter_map(|v| v.as_ref()));
    }

    url
}

pub async fn maybe_get_base_js(
    client: &reqwest::Client,
    opt_base_js_url: Option<String>,
) -> Result<Option<Bytes>> {
    if let Some(base_js_url) = opt_base_js_url {
        let url = format!("https://youtube.com{}", &base_js_url);
        let res = client.get(url).send().await?;
        let bytes_vec = res.bytes().await?;
        Ok(Some(bytes_vec))
    } else {
        Ok(None)
    }
}
