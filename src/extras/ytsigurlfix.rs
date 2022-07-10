use crate::extras::nfuncextract::extract_n_function;
use crate::model;
use crate::stderr;
use crate::zeroerror::Result;
use crate::zeroerror::ResultExt;
use crate::zeroerror::YoutubezeroError;
use crate::ytzero::IsAudioVideo;
use bytes::Bytes;

pub async fn fix_format_url_sig_n(
    isav: IsAudioVideo,
    format: &mut model::AdaptiveFormat,
    base_js_bytes: Option<Bytes>,
) -> Result<()> {
    let url_str_ref = &mut format.url;
    if let Some(url_str) = url_str_ref.as_ref() {
        let result = fix_format_url_str(isav, url_str, base_js_bytes)
            .await
            .chain_err(|| format!("{} fix_format_url_str", isav))?;
        *url_str_ref = Some(result);
    } else {
        format.non_live_url = Some(fix_non_live_url(isav, format).await?);
    }
    //tokio::time::sleep(Duration::from_millis(200)).await;

    Ok(())
}

pub async fn fix_format_url_str(
    isav: IsAudioVideo,
    url_str: &str,
    base_js_bytes: Option<Bytes>,
) -> Result<String> {
    let mut url = reqwest::Url::parse(url_str)?;
    let query = url
        .query()
        .ok_or("query not found url")?
        .as_bytes()
        .to_vec();

    let target_param_name = "n";
    {
        let mut pairs = url.query_pairs_mut();
        pairs.clear();
        for (k, v) in url::form_urlencoded::parse(&query) {
            if k == target_param_name {
                stderr!("{} token key:{} value:{}\n", isav, k, v);
                let target_param_value = v;
                let token = get_live_n_token(&target_param_value, base_js_bytes.as_ref()).await?;
                stderr!("{} token generated: {:?}\n", isav, token);

                pairs.append_pair(&k, &token);
            } else {
                pairs.append_pair(&k, &v);
            }
        }
    }

    //tokio::time::sleep(Duration::from_millis(200)).await;

    Ok(url.to_string())
}

// async fn get_live_n_token(token_input: &str) -> Result<String> {
//     super::nodejsrun::run_script_node(token_input, include_bytes!("ytspeedfix.js")).await
// }
// async fn get_non_live_sig_token(token_input: &str) -> Result<String> {
//     super::nodejsrun::run_script_node(token_input, include_bytes!("ytsigfix.js")).await
// }

async fn get_live_n_token(token_input: &str, base_js_bytes: Option<&Bytes>) -> Result<String> {
    let maybe_code = base_js_bytes.map(|b| extract_n_function(&b)).flatten();
    let n_code = if let Some(ref b) = maybe_code {
        b
    } else {
        &include_bytes!("ytspeedfix.js")[..]
    };

    super::boajsrun::run_script_boa(token_input, n_code).map_err(|s| s.into())
}
async fn get_non_live_sig_token(token_input: &str) -> Result<String> {
    super::boajsrun::run_script_boa(token_input, include_bytes!("ytsigfix.js"))
        .map_err(|s| s.into())
}

pub async fn fix_non_live_url(
    isav: IsAudioVideo,
    format: &model::AdaptiveFormat,
) -> Result<String> {
    let mut sig: Option<String> = None;
    let signature_cipher = format
        .signatureCipher
        .as_ref()
        .ok_or(YoutubezeroError::UrlNotFoundForItag)?;

    let mut urlstr = String::new();
    for (k, v) in url::form_urlencoded::parse(signature_cipher.as_bytes()) {
        //stderr!("{} key:{}, value:{}\n", isav, k, v);
        if k == "url" {
            urlstr = v.to_string();

            urlstr = crate::extras::ytsigurlfix::fix_format_url_str(isav, &urlstr, None).await?;
        } else if k == "s" {
            // signature?
            sig = Some(v.to_string());
        }
    }

    if let Some(sig) = sig {
        let sig_generated = get_non_live_sig_token(&sig).await?;
        //stderr!("{} sig_generated: {:?}", isav, sig_generated);
        let mut url = reqwest::Url::parse(&urlstr)?;
        {
            let mut pairs = url.query_pairs_mut();
            pairs.append_pair("sig", &sig_generated);
        }
        urlstr = url.to_string();
    }

    Ok(urlstr)
}
