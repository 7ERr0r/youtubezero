use regex::bytes::Regex;

pub fn extract_n_function_name(base_js: &[u8]) -> Option<String> {
    // a.u="";a.url="";a.D&&(b=a.get("n"))&&(b=Iw[0](b),a.set("n",b),Iw.length||zo(""))}};
    let re = Regex::new(
        r#"\.get\("n"\)\)&&\(b=(?P<nfunc>[a-zA-Z0-9$]+)(?:\[(?P<idx>\d+)\])?\([a-zA-Z0-9]\)"#,
    )
    .unwrap();
    if let Some(cap) = re.captures_iter(base_js).next() {
        let nfunc = &cap["nfunc"];
        let _idx = &cap["idx"];
        let nfunc = String::from_utf8_lossy(nfunc).into_owned();
        // /println!("found: nfunc:{} idx:{}", nfunc, idx);
        // var Iw=[zo];
        let re_str = format!(
            r#"var {nfunc}\s*=\s*(\[.+?\]);"#,
            nfunc = regex::escape(&nfunc)
        );
        let re2 = Regex::new(&re_str).unwrap();
        if let Some(cap) = re2.captures_iter(base_js).next() {
            return Some(String::from_utf8_lossy(&cap[1]).into_owned());
        };
    };
    None
}

pub fn extract_n_function(base_js: &[u8]) -> Option<Vec<u8>> {
    let name = extract_n_function_name(base_js);
    if let Some(name) = name {
        let name = name.trim_start_matches("[");
        let name = name.trim_end_matches("]");
        println!("extract_n_function found: name:{:?}", name);

        let re_code = r#"(?x)
        (?:function\s{1,2}funcname|[{;,]\s{0,2}funcname\s{0,2}=\s{0,2}function|var\s{1,2}funcname\s{0,2}=\s{0,2}function)\s{0,2}
        \((?P<args>[^)]{0,2})\)\s{0,2}
        (?P<code>\{(?:[^"]|"([^"]|\\"){0,10}")+catch\(d\)\{.{10,80}.join\(""\)\};)"#;
        let re_code = re_code.replace(" ", "").replace("funcname", name);
        let re_code = Regex::new(&re_code).unwrap();
        if let Some(cap_code) = re_code.captures_iter(base_js).next() {
            let code = &cap_code["code"];
            let args = &cap_code["args"];
            // println!("found: code:{}", code);
            // println!("found: args:{}", args);

            let glued_function = format!(
                "function encrypt({}){}",
                String::from_utf8_lossy(args),
                String::from_utf8_lossy(code)
            );
            //println!("final: {}", glued_function);
            return Some(glued_function.as_bytes().to_vec());
        };
    }
    None
}

pub fn extract_base_js_url(watchvindex_html: &[u8]) -> Option<String> {
    // <script src="/s/player/01234abcd/player_ias.vflset/en_US/base.js" nonce="abcABC_1234"></script>

    let re = regex::bytes::Regex::new(r#"<script\ssrc="((?:[/a-zA-Z0-9_\-\.]){1,100}base\.js)""#)
        .unwrap();
    if let Some(cap) = re.captures_iter(watchvindex_html).next() {
        return Some(String::from_utf8_lossy(&cap[1]).into_owned());
    };
    None
}

// #[test]
// fn test_base_js() {
//     let base = include_bytes!("base.js");

//     extract_n_function(&base[..]);
// }

#[test]
fn test_base_js_url_extract() {
    let base = br#"<script src="/s/player/01234abcd/player_ias.vflset/en_US/base.js" nonce="abcABC_1234"></script>"#;

    let res = extract_base_js_url(&base[..]);
    println!("{:?}", res);
}
