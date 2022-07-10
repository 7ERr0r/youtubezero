#![allow(non_snake_case)]

use serde::{Deserialize, Serialize};

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct AdaptiveFormat {
    pub itag: i64,
    pub url: Option<String>,
    pub bitrate: Option<i64>,
    pub mimeType: String,
    pub signatureCipher: Option<String>,
    pub contentLength: Option<String>,
    // custom-filled
    pub non_live_url: Option<String>,
    pub content_length: Option<i64>,
}

#[derive(Default, Serialize, Deserialize, Debug)]
pub struct StreamingData {
    pub adaptiveFormats: Vec<AdaptiveFormat>,
}

#[derive(Default, Serialize, Deserialize, Debug)]
pub struct VideoDetails {
    pub title: String,
    pub videoId: String,
    pub isLowLatencyLiveStream: Option<bool>,
    pub latencyClass: Option<String>,
    pub isLive: Option<bool>,
}

#[derive(Default, Serialize, Deserialize, Debug)]
pub struct PlayabilityStatus {
    pub status: Option<String>,
    pub reason: Option<String>,
}

#[derive(Default, Serialize, Deserialize, Debug)]
pub struct PlayerResponse {
    pub videoDetails: VideoDetails,
    pub streamingData: Option<StreamingData>,
    pub playabilityStatus: Option<PlayabilityStatus>,
}
// ytInitialPlayerResponse
// #[derive(Serialize, Deserialize, Debug)]
// pub struct InitialPlayerResponse {
//     pub responseContext: PlayerResponse,
// }

impl std::fmt::Display for AdaptiveFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {:?} / {}", self.itag, self.bitrate, self.mimeType)
    }
}

impl AdaptiveFormat {
    // MP4 container should be transcoded for ffplay
    // because it does not support seeking and speeding up
    pub fn should_transcode_to_mpegts(&self) -> bool {
        Self::is_mp4_itag(self.itag as i32)
            || self.mimeType.starts_with("video/mp4")
            || self.mimeType.starts_with("audio/mp4")
    }

    // What we really want to check is
    // if it's the MP4 container which freezes ffplay
    // mov,mp4,m4a,3gp,3g2,mj2
    pub fn is_mp4_itag(itag: i32) -> bool {
        (133..=137).contains(&itag) || itag == 160 || (298..=299).contains(&itag)
    }

    pub fn fix_content_length(&mut self) {
        self.content_length = self
            .contentLength
            .as_ref()
            .map(|s| s.as_str())
            .unwrap_or(&"")
            .parse()
            .ok()
    }

    pub fn get_live_or_offlive_url(&self) -> Option<&str> {
        if let Some(url) = &self.url {
            Some(url.as_str())
        } else {
            self.non_live_url.as_ref().map(|s| s.as_str())
        }
    }
}

#[derive(Clone, Default)]
pub struct OptionAudioVideo {
    pub audio: Option<AdaptiveFormat>,
    pub video: Option<AdaptiveFormat>,
}
#[derive(Clone, Default)]
pub struct AudioVideo {
    pub audio: AdaptiveFormat,
    pub video: AdaptiveFormat,
}

pub fn get_choose_formats(
    formats: &Vec<AdaptiveFormat>,
    itag_a: Option<i32>,
    itag_v: Option<i32>,
) -> OptionAudioVideo {
    let audio = get_best_single_format(formats, "audio/", itag_a);
    let video = get_best_single_format(formats, "video/", itag_v);
    OptionAudioVideo {
        audio: audio.map(|v| v.clone()),
        video: video.map(|v| v.clone()),
    }
}

pub fn get_best_single_format<'a>(
    formats: &'a Vec<AdaptiveFormat>,
    prefix: &str,
    itag: Option<i32>,
) -> Option<&'a AdaptiveFormat> {
    let mut best: Option<&AdaptiveFormat> = None;
    for format in formats {
        if format.mimeType.starts_with(prefix) {
            if let Some(itag) = itag {
                if format.itag == itag as i64 {
                    return Some(format);
                }
            }
            match best {
                None => {
                    best = Some(format);
                }
                Some(old) => {
                    if old.bitrate < format.bitrate {
                        best = Some(format);
                    }
                }
            }
        }
    }
    best
}

impl OptionAudioVideo {
    pub fn audio_video_or_err(self) -> crate::Result<AudioVideo> {
        Ok(AudioVideo {
            audio: self.audio.ok_or("best audio not found")?,
            video: self.video.ok_or("best video not found")?,
        })
    }
}
