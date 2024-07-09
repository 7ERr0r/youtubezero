pub mod extras;
pub mod macros;
pub mod model;
pub mod outwriter;
pub mod segmenter;
pub mod youtube;
pub mod ytzero;
pub mod zeroerror;

use core::time::Duration;

use error_chain::ChainedError;

use tokio::runtime::Runtime;
use zeroerror::Result;

use clap::Parser;

/// Outputs, video url, segment range and more
#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
pub struct YoutubezeroArgs {
    /// Video output: 'stdout' or 'video.mp4' or 'tcp:127.0.0.1:2000' or 'unix:/tmp/video.sock'
    #[clap(short, long, default_value = "video.mp4")]
    vout: Vec<String>,

    /// Audio output: 'stdout' or 'audio.mp4' or 'tcp:127.0.0.1:2001' or 'unix:/tmp/audio.sock'
    #[clap(short, long, default_value = "audio.mp4")]
    aout: Vec<String>,

    /// Youtube watch?v= url or path to 'file://./index_watch.html'
    #[clap(short, long)]
    url: String,

    /// True to follow head of live (latest segment)
    #[clap(short, long)]
    follow_head_seqnum: bool,

    /// First segment number to download
    #[clap(short, long, default_value_t = 0)]
    start_seqnum: i64,

    /// Last segment number to download
    #[clap(short, long)]
    end_seqnum: Option<i64>,

    /// How many attempts to fetch one segment number
    #[clap(short, long, default_value_t = 5)]
    retries: u16,

    /// True to disable low latency and downloads only confirmed, full segments
    #[clap(short, long)]
    whole_segments: bool,

    /// Max segments in flight
    #[clap(short, long, default_value_t = 20)]
    max_in_flight: u16,

    /// Audio format, eg. 251 for opus
    #[clap(long, default_value = "best")]
    aformat: String,

    /// Video format, eg. 134
    #[clap(long, default_value = "best")]
    vformat: String,

    /// Timeout of a single request
    #[clap(short, long, value_parser = parse_duration, default_value = "40")]
    timeout_one_request: Duration,

    /// True to save and cache valid segments on disk (when true enforces --whole-segments)
    #[clap(short, long)]
    cache_segments: bool,

    /// True to truncate and overwrite output files
    #[clap(long)]
    truncate: bool,

    /// Used to write debug/player_response.json
    #[clap(long)]
    write_debug_files: bool,
}

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let mut args = YoutubezeroArgs::parse();

    {
        clear_nulls(&mut args.vout);
        clear_nulls(&mut args.aout);
        let range = args.start_seqnum..=args.end_seqnum.unwrap_or_default();
        replace_out_name_template(&mut args.aout, &range);
        replace_out_name_template(&mut args.vout, &range);
        if args.cache_segments {
            args.whole_segments = true;
        }
    }

    // Create the runtime
    let rt = Runtime::new()?;

    // Spawn the root task
    let result = rt.block_on(ytzero::run_async(&args));
    if let Err(err) = result {
        println!("{}", err.display_chain().to_string());
    }
    Ok(())
}

fn parse_duration(
    arg: &str,
) -> std::result::Result<std::time::Duration, std::num::ParseFloatError> {
    let seconds: f64 = arg.parse()?;
    Ok(std::time::Duration::from_micros((seconds * 1.0e6) as u64))
}

fn clear_nulls(out_names: &mut Vec<String>) {
    out_names.retain(|f| !is_null_file(&f))
}
fn is_null_file(out_name: &str) -> bool {
    out_name.starts_with("/dev/null")
        || out_name.starts_with("none")
        || out_name.starts_with("null")
}
fn replace_out_name_template(out_names: &mut Vec<String>, range: &std::ops::RangeInclusive<i64>) {
    let start = range.start().to_string();
    let end = range.end().to_string();
    out_names
        .iter_mut()
        .for_each(|name| *name = name.replace("{start}", &start).replace("{end}", &end));
}
