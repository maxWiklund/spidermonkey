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
use serde::Deserialize;
use std::fs;
use std::path::PathBuf;
use tantivy::{Result as TantivyResult, TantivyError};

use std::process::Command;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub scan_settings: ScanSettings,
}

#[derive(Debug, Deserialize)]
pub struct ScanSettings {
    pub rescan_interval: Option<String>,
    pub pre_scan_commands: Option<Vec<String>>,
    pub scan_directory: Option<String>,
    pub exclude_patterns: Option<Vec<String>>,
    pub endpoint: Option<String>,
}

pub fn read_config(path: PathBuf) -> TantivyResult<Config> {
    let contents = fs::read_to_string(path)?;

    let config = match serde_yaml::from_str::<Config>(&contents) {
        Ok(cfg) => cfg,
        Err(e) => {
            return Err(TantivyError::InvalidArgument(format!(
                "Failed to parse YAML: {}",
                e
            )))
        }
    };
    Ok(config)
}

pub async fn execute_pre_scan_commands(commands: Vec<String>, cwd: String) -> TantivyResult<()> {
    for command_str in commands {
        let parts = match shell_words::split(&command_str) {
            Ok(p) => p,
            Err(e) => {
                return Err(TantivyError::InvalidArgument(format!(
                    "Failed to parse command '{}': {}",
                    command_str, e
                )))
            }
        };

        if parts.is_empty() {
            continue;
        }

        let (cmd, args) = parts.split_first().unwrap();
        let status = Command::new(cmd)
            .args(args)
            .current_dir(&cwd)
            .status()
            .map_err(|e| {
                TantivyError::InvalidArgument(format!("Failed to run '{}': {}", cmd, e))
            })?;

        if !status.success() {
            return Err(TantivyError::InvalidArgument(format!(
                "Command '{}' exited with status {}",
                command_str,
                status.code().unwrap_or(-1)
            )));
        }
    }

    Ok(())
}
