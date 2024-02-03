use error_chain::error_chain;
error_chain! {
    foreign_links {
        Fmt(::std::fmt::Error);
        Io(::std::io::Error);
        Url(url::ParseError);
        TokioJoin(tokio::task::JoinError);
        Youtubezero(YoutubezeroError);
        Reqwest(reqwest::Error);
        SerdeJson(serde_json::Error);
        ReqwestHeader(reqwest::header::InvalidHeaderValue);
        ParseInt(std::num::ParseIntError);
    }

}

#[derive(Debug, derive_more::Display)]
pub enum YoutubezeroError {
    #[display(fmt = "VideoParamNotProvided")]
    VideoParamNotProvided,
    #[display(fmt = "PlayerResponseNotFound")]
    PlayerResponseNotFound,

    #[display(fmt = "SegmentDataSendError")]
    SegmentDataSendError,

    #[display(fmt = "SegmentEventSendError")]
    SegmentEventSendError,

    #[display(fmt = "SegmentSendOutError")]
    SegmentSendOutError,

    #[display(fmt = "NoContentTypeHeader")]
    NoContentTypeHeader,

    #[display(fmt = "NoUnixSocketError: target_os != linux")]
    NoUnixSocketError,

    #[display(fmt = "UrlNotFoundForItag")]
    UrlNotFoundForItag,

    RunningTasksIsZeroBug,

    DeadlockFromHeadBug,
}
impl std::error::Error for YoutubezeroError {}
