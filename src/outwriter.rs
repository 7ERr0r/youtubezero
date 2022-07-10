use crate::zeroerror::ResultExt;
use crate::ytzero::IsAudioVideo;
use crate::ytzero::SessionConsts;
use bytes::Bytes;
use error_chain::ChainedError;
use std::convert::TryInto;
use tokio::fs::OpenOptions;
use tokio::task::JoinHandle;

use futures::future::FutureExt;
use std::cell::RefCell;
use std::rc::Rc;

use crate::zeroerror::Result;
use tokio::io::AsyncWrite;
use tokio::io::{self, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::Sender;

use crate::model;
use crate::stderr_result;

pub struct OutputStreamSender {
    pub reliable: bool,
    pub tx: Sender<Bytes>,
}

pub fn make_out_writers(
    isav: IsAudioVideo,
    outs: Vec<AsyncOutput>,
    copy_ended: Rc<RefCell<bool>>,
) -> (Vec<OutputStreamSender>, Vec<JoinHandle<()>>) {
    let mut txbufs = Vec::new();
    let mut handles = Vec::new();
    for mut out in outs {
        let channel_size = 1024;
        let (txbuf, rxbuf) = mpsc::channel::<Bytes>(channel_size);
        let _copy_endedc = copy_ended.clone();

        txbufs.push(OutputStreamSender {
            tx: txbuf,
            reliable: out.reliable,
        });
        handles.push(tokio::task::spawn_local(async move {
            let res = copy_channel_to_out(rxbuf, &mut out).await;
            match res {
                Err(err) => {
                    let _ = stderr_result!("{} copy_channel_to_out err: {}\n", isav, err);
                }
                Ok(_) => {}
            }
            let _ = stderr_result!("{} copy_channel_to_out ending\n", isav);
            //*copy_endedc.borrow_mut() = true;
        }));
    }
    (txbufs, handles)
}

pub struct AsyncOutput {
    pub writer: Box<dyn AsyncWrite + Unpin>,
    pub reliable: bool,
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

pub async fn make_outs(
    isav: IsAudioVideo,
    out_names: &[String],
    copy_ended: Rc<RefCell<bool>>,
    consts: Option<&Rc<SessionConsts>>,
    format: Option<&model::AdaptiveFormat>,
) -> Result<Vec<AsyncOutput>> {
    let mut out_names = out_names.to_vec();
    let mut outs: Vec<AsyncOutput> = Vec::new();

    while let Some(out_name) = out_names.pop() {
        match out_name.as_str() {
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
                out_names.push("ffvideo".to_string());
                out_names.push("ffaudio".to_string());
            }
            "ffvideo" => {
                let transcode_mpegts = format
                    .map(|f| f.should_transcode_to_mpegts())
                    .unwrap_or_default();
                start_with_ffplay(
                    IsAudioVideo::video(),
                    isav,
                    &copy_ended,
                    &mut outs,
                    consts,
                    transcode_mpegts,
                )?;
            }
            "ffaudio" => {
                let transcode_mpegts = format
                    .map(|f| f.should_transcode_to_mpegts())
                    .unwrap_or_default();
                start_with_ffplay(
                    IsAudioVideo::audio(),
                    isav,
                    &copy_ended,
                    &mut outs,
                    consts,
                    transcode_mpegts,
                )?;
            }
            other_out => {
                const TCP_O: &str = "tcp:";
                const UNIX_O: &str = "unix:";
                if let Some(addr) = other_out.strip_prefix(TCP_O) {
                    let addr = addr.strip_prefix("//").unwrap_or(addr);
                    use tokio::net::TcpStream;
                    let stream = TcpStream::connect(addr)
                        .await
                        .chain_err(|| format!("TcpStream::connect {}", addr))?;
                    outs.push(AsyncOutput::new(Box::new(stream)));
                } else if let Some(addr) = other_out.strip_prefix(UNIX_O) {
                    #[cfg(target_os = "linux")]
                    {
                        let addr = addr.strip_prefix("//").unwrap_or(addr);
                        use tokio::net::UnixStream;

                        let stream = UnixStream::connect(addr)
                            .await
                            .chain_err(|| format!("UnixStream::connect {}", addr))?;
                        outs.push(AsyncOutput::new(Box::new(stream)));
                    }
                    #[cfg(not(target_os = "linux"))]
                    {
                        return Err(YoutubelinkError::NoUnixSocketError.into());
                    }
                } else {
                    let open_res = OpenOptions::new()
                        .read(true)
                        .append(true)
                        .create(true)
                        .open(other_out)
                        .await;
                    let file = open_res.chain_err(|| format!("File::create {:?}", other_out))?;
                    outs.push(AsyncOutput::new(Box::new(file)));
                }
            }
        }
    }
    Ok(outs)
}

pub fn start_with_ffplay(
    onlyif: IsAudioVideo,
    isav: IsAudioVideo,
    copy_ended: &Rc<RefCell<bool>>,
    outs: &mut Vec<AsyncOutput>,
    consts: Option<&Rc<SessionConsts>>,
    transcode_mpegts: bool,
) -> Result<()> {
    if isav == onlyif {
        let title = consts
            .as_ref()
            .map(|c| c.player.videoDetails.title.as_str())
            .unwrap_or(&"live");
        let title = &title[..title.len().min(30)];
        if transcode_mpegts {
            let (ffstdin, ffstdout) = spawn_ffmpeg_mpegts(&copy_ended, isav)?;
            spawn_ffplay(&copy_ended, title, isav, Some(ffstdout))?;
            outs.push(AsyncOutput::new(Box::new(ffstdin)));
        } else {
            let playstdin = spawn_ffplay(&copy_ended, title, isav, None)?;

            // Unreliable because it drops packets / frames
            // We can drop mpegts frames
            // But we can not drop mp4 (webm probably too)
            outs.push(AsyncOutput::new_unreliable(Box::new(playstdin.unwrap())));
        }
    }
    Ok(())
}

pub async fn copy_channel_to_out(mut rxbuf: Receiver<Bytes>, out: &mut AsyncOutput) -> Result<()> {
    loop {
        // Give some time to other tasks
        tokio::task::yield_now().await;

        // We'll receive from channel
        // in the future
        let f = rxbuf.recv();

        let once_msg: Option<Option<Bytes>> = f.now_or_never();
        let msg: Option<Bytes>;
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
                out.writer.flush().await?;
                out.writer.shutdown().await?;
                break;
            }
        }
    }
    Ok(())
}

fn spawn_ffmpeg_mpegts(
    copy_ended: &Rc<RefCell<bool>>,
    isav: IsAudioVideo,
) -> Result<(tokio::process::ChildStdin, tokio::process::ChildStdout)> {
    use std::process::Stdio;
    use tokio::process::Command;

    let mut cmd = Box::new(Command::new("ffmpeg"));

    cmd.stderr(Stdio::null())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());

    // ffmpeg -i - -c:v copy -c:a copy -f mpegts -
    cmd.arg("-i")
        .arg("-")
        .arg("-c")
        .arg("copy")
        .arg("-f")
        .arg("mpegts")
        .arg("-");

    let mut child = cmd.spawn().chain_err(|| "failed to spawn ffmpeg")?;

    let stdin = child
        .stdin
        .take()
        .chain_err(|| "child ffmpeg did not have a handle to stdin")?;

    let stdout = child
        .stdout
        .take()
        .chain_err(|| "child ffmpeg did not have a handle to stdout")?;

    let process_ended = copy_ended.clone();
    tokio::task::spawn_local(async move {
        let result = child
            .wait()
            .await
            .chain_err(|| "child ffmpeg process encountered an error");
        match result {
            Err(err) => {
                let _ = stderr_result!(
                    "{} child ffmpeg wait result was: {}\n",
                    isav,
                    err.display_chain()
                );
            }
            Ok(status) => {
                let _ = stderr_result!("{} child ffmpeg status was: {}\n", isav, status);
            }
        }
        *process_ended.borrow_mut() = true;
    });

    Ok((stdin, stdout))
}

fn spawn_ffplay(
    copy_ended: &Rc<RefCell<bool>>,
    channel: &str,
    isav: IsAudioVideo,
    stdio_in: Option<tokio::process::ChildStdout>,
) -> Result<Option<tokio::process::ChildStdin>> {
    use std::process::Stdio;
    use tokio::process::Command;

    let mut cmd = Box::new(Command::new("ffplay"));

    // audio
    // ffplay -window_title "%channel%" -fflags nobuffer -flags low_delay -af volume=0.2 -x 1280 -y 720 -i -

    // video
    // ffplay -window_title "%channel%" -probesize 32 -sync ext -af volume=0.3 -vf setpts=0.5*PTS -x 1280 -y 720 -i -

    if isav.is_audio() {
        cmd.arg("-window_title").arg(format!("{} - audio", channel));
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
            .arg(format!("{} - youtubezero", channel));

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

    cmd.stderr(Stdio::null());

    let chained_in = stdio_in.is_some();
    if let Some(stdio_in) = stdio_in {
        let stdio: std::process::Stdio = stdio_in.try_into()?;
        cmd.stdin(stdio);
    } else {
        cmd.stdin(Stdio::piped());
    };

    let mut child = cmd.spawn().chain_err(|| "failed to spawn ffplay")?;

    let mut stdin_result = None;
    if !chained_in {
        let stdin = child
            .stdin
            .take()
            .chain_err(|| "child ffplay did not have a handle to stdin")?;
        stdin_result = Some(stdin);
    }

    let process_ended = copy_ended.clone();
    tokio::task::spawn_local(async move {
        let result = child
            .wait()
            .await
            .chain_err(|| "child ffplay process encountered an error");
        match result {
            Err(err) => {
                let _ = stderr_result!("child ffplay wait result was: {}\n", err.display_chain());
            }
            Ok(status) => {
                let _ = stderr_result!("child ffplay status was: {}\n", status);
            }
        }
        *process_ended.borrow_mut() = true;
    });

    Ok(stdin_result)
}
