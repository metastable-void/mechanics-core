
use mechanics_core::{RuntimeState, Runtime};

fn main() -> std::io::Result<()> {
    let args = std::env::args().collect::<Vec<_>>();
    let (config_path, js_path) = match (args.get(1), args.get(2)) {
        (Some(p), Some(p2)) => (p, p2),
        _ => {
            println!("Usage: {} <json_config_path> <js_path>", &args[0]);
            return Ok(());
        },
    };
    let config_json = std::fs::read_to_string(config_path)?;
    let js_source = std::fs::read_to_string(js_path)?;
    let config: RuntimeState = serde_json::from_str(&config_json).map_err(|e| std::io::Error::other(e))?;
    let runtime = Runtime::new(config);
    let value = runtime.run_source(&js_source, serde_json::Value::Null).map_err(std::io::Error::other)?;
    let json = serde_json::to_string_pretty(&value).map_err(std::io::Error::other)?;
    println!("{}", &json);
    Ok(())
}
