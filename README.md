youtubezero
===
Zero latency youtube live downloader

![screenshot](https://raw.githubusercontent.com/Szperak/youtubezero/main/screenshot.png)


```
youtubezero 0.2.0
Outputs, video url, segment range and more

USAGE:
    youtubezero [OPTIONS] --url <URL>

OPTIONS:
    -a, --aout <AOUT>
            Audio output: 'stdout' or 'audio.mp4' or 'tcp:127.0.0.1:2001' or 'unix:/tmp/audio.sock'
            [default: audio.mp4]

        --aformat <AFORMAT>
            Audio format, eg. 251 for opus [default: best]

    -c, --cache-segments
            True to save and cache valid segments on disk (when true enforces --whole-segments)

    -e, --end-seqnum <END_SEQNUM>
            Last segment number to download

    -f, --follow-head-seqnum
            True to follow head of live (latest segment)

    -h, --help
            Print help information

    -m, --max-in-flight <MAX_IN_FLIGHT>
            Max segments in flight [default: 20]

    -r, --retries <RETRIES>
            How many attempts to fetch one segment number [default: 5]

    -s, --start-seqnum <START_SEQNUM>
            First segment number to download [default: 0]

    -t, --timeout-one-request <TIMEOUT_ONE_REQUEST>
            Timeout of a single request [default: 40]

    -u, --url <URL>
            Youtube watch?v= url or path to 'file://./index_watch.html'

    -v, --vout <VOUT>
            Video output: 'stdout' or 'video.mp4' or 'tcp:127.0.0.1:2000' or 'unix:/tmp/video.sock'
            [default: video.mp4]

    -V, --version
            Print version information

        --vformat <VFORMAT>
            Video format [default: best]

    -w, --whole-segments
            True to disable low latency and downloads only confirmed, full segments
```

```
cargo run --release -- -u "https://www.youtube.com/watch?v=live" -v "tcp:127.0.0.1:2000" -a "tcp:127.0.0.1:2001"
```

for lowest latency:
```
cargo run --release -- -f -u "https://www.youtube.com/watch?v=live" -v ffvideo -a ffaudio
```