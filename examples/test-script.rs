use std::sync::Arc;

use mechanics_core::{MechanicsConfig, MechanicsJob, MechanicsPool, MechanicsPoolConfig};

fn main() -> std::io::Result<()> {
    let args = std::env::args().collect::<Vec<_>>();
    let (config_path, js_path) = match (args.get(1), args.get(2)) {
        (Some(p), Some(p2)) => (p, p2),
        _ => {
            #[allow(clippy::get_first)]
            let bin_name = args.get(0).map(String::as_str).unwrap_or("test-script");
            println!(
                "Usage: {} <json_config_path> <js_path>",
                bin_name
            );
            return Ok(());
        }
    };
    let config_json = std::fs::read_to_string(config_path)?;
    let js_source = std::fs::read_to_string(js_path)?;
    let config: MechanicsConfig =
        serde_json::from_str(&config_json).map_err(std::io::Error::other)?;
    let pool = MechanicsPool::new(MechanicsPoolConfig::default()).map_err(std::io::Error::other)?;
    let job = MechanicsJob {
        mod_source: js_source.into(),
        arg: Arc::new(serde_json::Value::Null),
        config: Arc::new(config),
    };
    let value = pool.run(job).map_err(std::io::Error::other)?;
    let json = serde_json::to_string_pretty(&value).map_err(std::io::Error::other)?;
    println!("{}", &json);
    Ok(())
}
