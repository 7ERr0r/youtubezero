youtubelink
===


```
USAGE:
    youtubelink.exe [OPTIONS] --url <url>   


OPTIONS:
    -a, --aout <aout>...    Audio output: 'stdout' or 'audio.mp4' or 'tcp:127.0.0.1:2001' or 'unix:/tmp/audio.sock'
    -u, --url <url>         Youtube watch?v= url
    -v, --vout <vout>...    Video output: 'stdout' or 'video.mp4' or 'tcp:127.0.0.1:2000' or 'unix:/tmp/video.sock'
```

```
cargo run --release -- --url "https://www.youtube.com/watch?v=live" -v "tcp:127.0.0.1:2000" -a "tcp:127.0.0.1:2001"
```

for lowest latency:
```
cargo run --release -- -f -u "https://www.youtube.com/watch?v=live" -v ffvideo -a ffaudio
```