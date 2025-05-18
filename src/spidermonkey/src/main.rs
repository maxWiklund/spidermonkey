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

use search_engine;
use tantivy::Result as TantivyResult;

use axum::{
    extract::{Query, State},
    http::Method,
    response::Json,
    routing::get,
    Router,
};
use search_engine::CodeSearchEngine;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

use tower_http::cors::{Any, CorsLayer};

use clap::{Arg, Command};

#[derive(Debug, Deserialize)]
struct SearchParams {
    text: String,
}

// Handler that accesses state and query parameters
async fn search_handler(
    State(search_engine): State<Arc<CodeSearchEngine>>,
    Query(params): Query<SearchParams>,
) -> Json<Value> {
    match search_engine.search(&params.text) {
        Ok(value) => match serde_json::to_value(value) {
            Ok(json_val) => Json(json_val),
            Err(_) => Json(json!({ "results": [] })),
        },
        Err(_) => Json(json!({ "results": [] })),
    }
}

fn build_cli() -> Command {
    Command::new("spidermonkey")
        .about("A rest api to index and search through the files. ")
        .arg(
            Arg::new("endpoint")
                .long("endpoint")
                .default_value("127.0.0.1:3000")
                .value_name("URL")
                .help("The endpoint URL to connect to"),
        )
        .arg(
            Arg::new("memory")
                .long("memory")
                // .short('m')
                .default_value("500")
                .value_name("MB")
                .help("The amount of memory to allocate in RAM for indexing.")
                .value_parser(clap::value_parser!(usize)),
        )
        .arg(
            Arg::new("directory")
                .long("directory")
                // .short("dir")
                .value_name("DIR")
                .help("Directory to index and search")
                .required(true),
        )
}

#[tokio::main]
async fn main() -> TantivyResult<()> {
    let matches = build_cli().get_matches();

    let endpoint = matches.get_one::<String>("endpoint").cloned().unwrap();
    let mb = matches.get_one::<usize>("memory").cloned().unwrap();
    let directory = matches
        .get_one::<String>("directory")
        .expect("directory is required")
        .to_owned();

    println!("Spidermonkey startup");

    let search_app = Arc::new(
        search_engine::CodeSearchEngine::new(directory.as_str(), mb * 1024 * 1024).unwrap(),
    );

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
    let listener = tokio::net::TcpListener::bind(endpoint).await.unwrap();
    println!("listening on {}", listener.local_addr().unwrap());
    axum::serve(listener, app).await?;
    Ok(())
}
