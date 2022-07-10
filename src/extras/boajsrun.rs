
use boa_engine::property::Attribute;
use boa_engine::Context;

pub fn run_script_boa(token_input: &str, script_file_bytes: &[u8]) -> Result<String, String> {
    let mut context = Context::default();
    // Parse the source code
    context
        .eval(script_file_bytes)
        .map_err(|err| format!("boa context.eval script_file_bytes: {}", err.display()))?;

    // Run encrypt
    context.register_global_property("token_input", token_input, Attribute::all());
    let result = context
        .eval("encrypt(token_input)")
        .map_err(|err| format!("boa context.eval encrypt(...): {}", err.display()))?;
    let result = result
        .to_string(&mut context)
        .map_err(|err| format!("boa result.to_string: {}", err.display()))?;

    let result = result.to_string();

    Ok(result)
}

#[test]
fn test_boa_script() {
    let result = run_script_boa("fen6sxRQg2Ws0MbTVUO", include_bytes!("ytspeedfix.js"));
    println!("{:?}", result);
}
