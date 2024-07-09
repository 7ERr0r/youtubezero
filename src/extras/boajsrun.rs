use boa_engine::property::Attribute;
use boa_engine::{js_string, Context, JsString, Source};

pub fn run_script_boa(token_input: &str, script_file_bytes: &[u8]) -> Result<String, String> {
    let mut context = Context::default();
    // Parse the source code

    let script_source = Source::from_bytes(script_file_bytes);
    context
        .eval(script_source)
        .map_err(|err| format!("boa context.eval script_file_bytes: {}", err))?;

    // Run encrypt
    context
        .register_global_property(
            js_string!("token_input"),
            JsString::from(token_input),
            Attribute::all(),
        )
        .map_err(|err| format!("boa register_global_property: {}", err))?;
    let encrypt_source = Source::from_bytes("encrypt(token_input)");
    let result = context
        .eval(encrypt_source)
        .map_err(|err| format!("boa context.eval encrypt(...): {}", err))?;
    let result: boa_engine::JsString = result
        .to_string(&mut context)
        .map_err(|err| format!("boa result.to_string: {}", err))?;

    let result = result
        .to_std_string()
        .map_err(|err| format!("boa result.to_std_string: {}", err))?;

    Ok(result)
}

#[test]
fn test_boa_script() {
    let result = run_script_boa("fen6sxRQg2Ws0MbTVUO", include_bytes!("ytspeedfix.js"));
    println!("{:?}", result);
}
