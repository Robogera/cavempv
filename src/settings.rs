use std::env;

use config::{Config, ConfigError, Environment, File};
use serde::Deserialize;

#[derive(Debug, Deserialize, Clone)]
#[allow(unused)]
pub struct Settings {
    pub log_dir: String,
    pub serial_port: String,
    pub baud_rate: usize,
    pub sleep_timeout_sec: usize,
    pub playlist: Vec<Fragment>,
}

#[derive(Debug, Deserialize, Clone)]
#[allow(unused)]
pub struct Fragment {
    pub intro: Option<String>,
    #[serde(rename = "static")]
    pub static_: String,
    pub fadeout: Option<Vec<Fadeout>>,
}
#[derive(Debug, Deserialize, Clone)]
#[allow(unused)]
pub struct Fadeout {
    pub before: Option<f32>,
    pub video: String,
}

impl Settings {
    pub fn new() -> Result<Self, ConfigError> {
        let config_name = env::var("CONFIG_FILE").unwrap_or_else(|_| "main".into());
        let s = Config::builder()
            .add_source(File::with_name(&format!("cfg/{config_name}")))
            .add_source(Environment::with_prefix("detect"))
            .build()?;
        s.try_deserialize()
    }
}
