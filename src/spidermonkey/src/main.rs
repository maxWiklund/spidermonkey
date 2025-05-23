// Copyright (C) 2025  Max Wiklund
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//   http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

mod config;

use humantime::parse_duration;

use axum::{
    extract::{Query, State},
    http::Method,
    response::Json,
    routing::get,
    Router,
};
use search_engine;
use search_engine::CodeSearchEngine;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use tantivy::{Result as TantivyResult, TantivyError};
use tokio::time::{sleep, Duration};
use tower_http::cors::{Any, CorsLayer};

use clap::{Arg, ArgGroup, Command};

#[derive(Debug, Deserialize)]
struct SearchParams {
    text: String,
}

async fn search_handler(
    State(search_engine): State<Arc<CodeSearchEngine>>,
    Query(params): Query<SearchParams>,
) -> Json<Value> {
    match search_engine.search(&params.text).await {
        Ok(value) => match serde_json::to_value(value) {
            Ok(json_val) => Json(json_val),
            Err(_) => Json(json!({ "results": [] })),
        },
        Err(_) => Json(json!({ "results": [] })),
    }
}

fn build_cli() -> Command {
    Command::new("spidermonkey")
        .about("A rest api to index and search through the files.")
        .arg(
            Arg::new("endpoint")
                .long("endpoint")
                .short('e')
                .value_name("URL")
                .help("The endpoint URL to connect to. e.g 127.0.0.1:3000"),
        )
        .arg(
            Arg::new("interval")
                .long("interval")
                .help("Interval between index and rebuild e.g (5s, 10m , 2h)"),
        )
        .arg(
            Arg::new("directory")
                .long("directory")
                .short('d')
                .value_name("DIR")
                .help("Directory to index and search"), // .required(true),
        )
        .arg(
            Arg::new("config")
                .long("config")
                .short('c')
                .help("File path to YAML config to load.")
                .value_parser(clap::value_parser!(PathBuf)),
        )
        .group(
            ArgGroup::new("input")
                .args(&["directory", "config"])
                .required(true), // Require one of the group
        )
}

#[tokio::main]
async fn main() -> TantivyResult<()> {
    let app_conf = exec_cli()?;

    println!("Spidermonkey startup");

    let search_app = Arc::new(
        CodeSearchEngine::new(app_conf.directory.as_str(), app_conf.exclude_patterns)
            .await
            .unwrap(),
    );
    let search_engine = Arc::new(search_app.clone());

    // Spawn a task to scan disk for changes every n seconds.
    tokio::spawn(async move {
        loop {
            sleep(app_conf.interval).await; // Wait for n seconds.
            let _ = config::execute_pre_scan_commands(
                app_conf.pre_scan_commands.clone(),
                app_conf.directory.clone(),
            )
            .await;
            // Execute command.
            match search_engine.reload(app_conf.directory.as_str()).await {
                Err(e) => {
                    eprintln!("{e:#}");
                }
                _ => {}
            }
        }
    });

    // Build CORS middleware
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::OPTIONS])
        .allow_headers(Any);

    // Pass state into the router
    let app = Router::new()
        .route("/search", get(search_handler))
        .with_state(search_app)
        .layer(cors);
    let listener = tokio::net::TcpListener::bind(app_conf.endpoint)
        .await
        .unwrap();
    println!("listening on {}", listener.local_addr().unwrap());
    axum::serve(listener, app).await?;
    Ok(())
}

#[derive(Debug)]
struct AppConfig {
    directory: String,
    endpoint: String,
    pre_scan_commands: Vec<String>,
    interval: Duration,
    exclude_patterns: Vec<String>,
}

impl AppConfig {
    fn new() -> Self {
        Self {
            directory: String::new(),
            endpoint: "127.0.0.1:3000".to_string(),
            pre_scan_commands: Vec::new(),
            interval: Duration::from_secs(30),
            exclude_patterns: vec![".git".to_string()],
        }
    }

    fn with_config(&mut self, settings: config::ScanSettings) -> &mut Self {
        if let Some(dir) = settings.scan_directory {
            self.directory = dir;
        }
        if let Some(ep) = settings.endpoint {
            self.endpoint = ep;
        }
        if let Some(cmds) = settings.pre_scan_commands {
            self.pre_scan_commands = cmds;
        }
        if let Some(intv) = settings.rescan_interval {
            if let Ok(dur) = parse_duration(&intv) {
                self.interval = dur;
            }
        }
        if let Some(excludes) = settings.exclude_patterns {
            self.exclude_patterns = excludes;
        }
        self
    }

    fn with_cli(&mut self, matches: &clap::ArgMatches) -> &mut Self {
        if let Some(dir) = matches.get_one::<String>("directory") {
            self.directory = dir.clone();
        }
        if let Some(endpoint) = matches.get_one::<String>("endpoint") {
            self.endpoint = endpoint.clone();
        }
        if let Some(interval) = matches.get_one::<String>("interval") {
            if let Ok(dur) = parse_duration(interval) {
                self.interval = dur;
            }
        }
        self
    }

    fn validate(&self) -> TantivyResult<()> {
        if self.directory.trim().is_empty() {
            return Err(TantivyError::InvalidArgument(
                "Directory path cannot be empty.".to_string(),
            ));
        }

        Ok(())
    }
}

fn exec_cli() -> TantivyResult<AppConfig> {
    let matches = build_cli().get_matches();
    let mut config = AppConfig::new();

    if let Some(config_path) = matches.get_one::<PathBuf>("config") {
        let conf = config::read_config(config_path.to_path_buf())?;
        config.with_config(conf.scan_settings);
    }

    config.with_cli(&matches);
    config.validate()?;
    Ok(config)
}
