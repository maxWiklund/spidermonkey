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

use serde::Serialize;
use std::sync::RwLock;

use sha2::{Digest, Sha256};
use std::time::Instant;
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{self, BufRead},
};
use tantivy::schema::Value;
use tantivy::{
    doc,
    schema::{Field, Schema, STORED, TEXT},
    Index, Result as TantivyResult, TantivyDocument, Term,
};
use walkdir::WalkDir;

use tokio::task;
#[derive(Debug, Serialize)]
pub struct LineRange {
    start: usize,
    end: usize,
}

#[derive(Debug, Serialize)]
pub struct SearchResult {
    body: String,
    path: String,
    line: usize,
    line_range: LineRange,
}

#[derive(Debug, Serialize)]
pub struct SearchResults {
    results: Vec<SearchResult>,
    time: f64,
}

#[derive(Clone)]
struct SearchFields {
    path: Field,
    line: Field,
    body: Field,
}

const DEFAULT_SEARCH_LIMIT: usize = 100_000_000;
const DEFAULT_MEMORY_SIZE: usize = 50_000_000;

fn calculate_checksum(file_path: &str) -> TantivyResult<String> {
    let file = fs::File::open(file_path)?;
    let mut reader = io::BufReader::new(file);
    let mut hasher = Sha256::new();
    io::copy(&mut reader, &mut hasher)?;
    Ok(format!("{:x}", hasher.finalize()))
}

fn find_file_paths(directory: &str, exclude_patterns: &Vec<String>) -> TantivyResult<Vec<String>> {
    let mut file_paths: Vec<String> = Vec::new();
    for entry in WalkDir::new(directory).into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.is_file() {
            if let Some(name) = path.to_str() {
                // Skip file if it matches any exclude pattern
                if exclude_patterns
                    .iter()
                    .any(|pattern| name.contains(pattern))
                {
                    continue;
                }
                file_paths.push(name.to_string());
            }
        }
    }
    Ok(file_paths)
}

async fn get_file_hashes(
    directory: &str,
    exclude_patterns: &Vec<String>,
) -> TantivyResult<HashMap<String, String>> {
    let paths = find_file_paths(directory, exclude_patterns)?;
    let mut handles = Vec::with_capacity(paths.len());

    // Spawn tasks for each file
    for path in paths {
        let handle =
            task::spawn_blocking(move || calculate_checksum(&path).map(|hash| (path, hash)));
        handles.push(handle);
    }

    // Collect results
    let mut hashes = HashMap::new();
    for handle in handles {
        if let Ok(Ok((path, hash))) = handle.await {
            hashes.insert(path, hash);
        }
    }

    Ok(hashes)
}

pub struct CodeSearchEngine {
    index: RwLock<Index>,
    fields: SearchFields,
    /// In-memory storage of all file lines by path
    lines_map: RwLock<HashMap<String, Vec<String>>>,
    file_hashes: RwLock<HashMap<String, String>>,
    exclude_patterns: Vec<String>,
}

impl CodeSearchEngine {
    /// Create a new search engine, build schema and index all files in directory
    pub async fn new(dir: &str, exclude_patterns: Vec<String>) -> TantivyResult<Self> {
        let mut schema_builder = Schema::builder();
        let path_field = schema_builder.add_text_field("path", TEXT | STORED);
        let line_field = schema_builder.add_i64_field("line", STORED);
        let body_field = schema_builder.add_text_field("body", TEXT | STORED);
        let schema = schema_builder.build();

        let index = Index::create_in_ram(schema.clone());
        let mut writer = index.writer(DEFAULT_MEMORY_SIZE)?;
        let mut lines_map: HashMap<String, Vec<String>> = HashMap::new();

        let start = Instant::now();
        let hashes = get_file_hashes(dir, &exclude_patterns).await?;

        for path in hashes.keys() {
            if let Ok(file) = fs::File::open(path) {
                let mut vec_lines: Vec<String> = Vec::new();
                for (num, line) in io::BufReader::new(file).lines().enumerate() {
                    if let Ok(text) = line {
                        // Index each line
                        writer.add_document(doc!(
                            path_field => path.clone(),
                            line_field => (num as i64 + 1),
                            body_field => text.clone(),
                        ))?;
                        vec_lines.push(text);
                    }
                }
                lines_map.insert(path.to_string(), vec_lines);
            }
        }
        let duration = start.elapsed();
        writer.commit()?;
        println!("Seconds to index all files: {}", duration.as_secs_f64());

        Ok(Self {
            index: RwLock::new(index),
            fields: SearchFields {
                path: path_field,
                line: line_field,
                body: body_field,
            },
            lines_map: RwLock::new(lines_map),
            file_hashes: RwLock::new(hashes),
            exclude_patterns: exclude_patterns,
        })
    }

    /// Execute a query and return matching results as JSON
    pub async fn search(&self, query_text: &str) -> TantivyResult<SearchResults> {
        let start = Instant::now();
        let index_read = self.index.read().unwrap(); // acquire the lock once
        let reader = index_read.reader_builder().try_into()?;
        let searcher = reader.searcher();

        let query_parser =
            tantivy::query::QueryParser::for_index(&index_read, vec![self.fields.body]);

        let query = query_parser.parse_query(query_text)?;
        let top_docs = searcher.search(
            &query,
            &tantivy::collector::TopDocs::with_limit(DEFAULT_SEARCH_LIMIT),
        )?;

        let mut found_results: Vec<SearchResult> = Vec::new();
        for (_score, doc_address) in top_docs {
            let retrieved: TantivyDocument = searcher.doc(doc_address)?;
            let file_path = retrieved
                .get_first(self.fields.path)
                .unwrap()
                .as_str()
                .unwrap();
            let line_num = retrieved
                .get_first(self.fields.line)
                .unwrap()
                .as_i64()
                .unwrap() as usize;

            if let Some((lines, (start, end))) = self.read_lines(file_path, line_num, 3) {
                found_results.push(SearchResult {
                    body: lines,
                    path: file_path.to_string(),
                    line: line_num,
                    line_range: LineRange { start, end },
                });
            }
        }

        let duration = start.elapsed();
        Ok(SearchResults {
            results: found_results,
            time: duration.as_secs_f64(),
        })
    }

    /// Helper method to read N lines around a target line from in-memory cache
    fn read_lines(
        &self,
        file_path: &str,
        line: usize,
        n: usize,
    ) -> Option<(String, (usize, usize))> {
        let binding = self.lines_map.read().unwrap();
        let file_lines = binding.get(file_path)?;
        let total = file_lines.len();
        if line > total {
            return None;
        }

        let start = line.saturating_sub(1).saturating_sub(n);
        let end = (line - 1 + n).min(total - 1);
        let snippet = file_lines[start..=end].join("\n");
        Some((snippet, (start + 1, end + 1)))
    }

    pub async fn reload(&self, directory: &str) -> TantivyResult<()> {
        let hashes = get_file_hashes(directory, &self.exclude_patterns).await?;
        let current_paths: HashSet<String> = hashes.keys().cloned().collect();

        let old_hashes_read = self.file_hashes.read().unwrap();
        let old_paths: HashSet<String> = old_hashes_read.keys().cloned().collect();
        drop(old_hashes_read); // Done reading

        // Determine missing files.
        let missing_files: Vec<String> = old_paths.difference(&current_paths).cloned().collect();

        let mut writer = self.index.write().unwrap().writer(DEFAULT_MEMORY_SIZE)?;

        // Add/update files
        for (path, hash) in &hashes {
            let should_update = {
                let file_hashes_read = self.file_hashes.read().unwrap();
                match file_hashes_read.get(path) {
                    Some(last_checksum) if last_checksum == hash => false,
                    _ => true,
                }
            };

            if !should_update {
                continue;
            }

            // Update hash
            {
                let mut file_hashes_write = self.file_hashes.write().unwrap();
                file_hashes_write.insert(path.clone(), hash.clone());
            }

            // Open file and index lines
            if let Ok(file) = fs::File::open(path) {
                let mut vec_lines = Vec::new();
                for (num, line) in io::BufReader::new(file).lines().enumerate() {
                    if let Ok(text) = line {
                        writer.add_document(doc!(
                            self.fields.path => path.clone(),
                            self.fields.line => (num as i64 + 1),
                            self.fields.body => text.clone(),
                        ))?;
                        vec_lines.push(text);
                    }
                }

                let mut lines_map_write = self.lines_map.write().unwrap();
                lines_map_write.insert(path.clone(), vec_lines);
            }
        }

        // Remove missing files
        if !missing_files.is_empty() {
            for path in &missing_files {
                let term = Term::from_field_text(self.fields.path, path);
                writer.delete_term(term);
            }

            let mut file_hashes_write = self.file_hashes.write().unwrap();
            let mut lines_map_write = self.lines_map.write().unwrap();
            for path in &missing_files {
                file_hashes_write.remove(path);
                lines_map_write.remove(path);
            }
        }

        writer.commit()?;
        Ok(())
    }
}
