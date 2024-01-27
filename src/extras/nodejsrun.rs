use crate::zeroerror::Result;

use rand::thread_rng;
use rand::Rng;
use std::process::Stdio;
use tokio::fs::File;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio::process::Command;

pub async fn run_script_node(token_input: &str, script_file_bytes: &[u8]) -> Result<String> {
    let rand_num: u32 = thread_rng().gen();
    let ytzero_js_path = format!("ytzero_script.{}.js", rand_num);
    {
        let mut file = File::create(&ytzero_js_path).await?;
        file.write_all(script_file_bytes).await?;
    }
    let result = run_node(token_input, &ytzero_js_path).await;

    let _ = tokio::fs::remove_file(&ytzero_js_path).await;

    result
}

async fn run_node(token_input: &str, ytzero_js_path: &str) -> Result<String> {
    let mut cmd = Command::new("node");
    cmd.arg(&ytzero_js_path);
    cmd.arg(&token_input);
    cmd.stdout(Stdio::piped());

    let mut child = cmd.spawn()?;

    let stdout = child
        .stdout
        .take()
        .ok_or("child did not have a handle to stdout")?;

    let mut reader = BufReader::new(stdout).lines();

    let mut final_token = None;
    while let Some(line) = reader.next_line().await? {
        if let Some(token) = line.strip_prefix("youtubezero_result:") {
            let token = token.trim_end();
            final_token = Some(token.to_string());
        }
    }
    let _status = child.wait().await?;

    let final_token = final_token.unwrap_or_else(|| String::from(token_input));

    Ok(final_token)
}
