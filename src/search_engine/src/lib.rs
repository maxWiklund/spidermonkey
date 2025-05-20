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

use std::time::Instant;
use std::{
    collections::HashMap,
    fs,
    io::{self, BufRead},
};
use tantivy::schema::Value;
use tantivy::{
    doc,
    schema::{Field, Schema, STORED, TEXT},
    Index, Result as TantivyResult, TantivyDocument,
};
use walkdir::WalkDir;

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

pub struct CodeSearchEngine {
    index: Index,
    fields: SearchFields,
    /// In-memory storage of all file lines by path
    lines_map: HashMap<String, Vec<String>>,
}

impl CodeSearchEngine {
    /// Create a new search engine, build schema and index all files in directory
    pub fn new(dir: &str, memory: usize) -> TantivyResult<Self> {
        let mut schema_builder = Schema::builder();
        let path_field = schema_builder.add_text_field("path", STORED);
        let line_field = schema_builder.add_i64_field("line", STORED);
        let body_field = schema_builder.add_text_field("body", TEXT | STORED);
        let schema = schema_builder.build();

        let index = Index::create_in_ram(schema.clone());
        let mut writer = index.writer(memory)?;
        let mut lines_map: HashMap<String, Vec<String>> = HashMap::new();

        for entry in WalkDir::new(dir).into_iter().filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.is_file() {
                if let Some(name) = path.to_str() {
                    if name.contains(".git/") {
                        continue;
                    }
                }
                if let Ok(file) = fs::File::open(path) {
                    let path_str = path.to_string_lossy().to_string();
                    let mut vec_lines: Vec<String> = Vec::new();
                    for (num, line) in io::BufReader::new(file).lines().enumerate() {
                        if let Ok(text) = line {
                            // Index each line
                            writer.add_document(doc!(
                                path_field => path_str.clone(),
                                line_field => (num as i64 + 1),
                                body_field => text.clone(),
                            ))?;
                            vec_lines.push(text);
                        }
                    }
                    lines_map.insert(path_str, vec_lines);
                }
            }
        }

        writer.commit()?;

        Ok(Self {
            index,
            fields: SearchFields {
                path: path_field,
                line: line_field,
                body: body_field,
            },
            lines_map,
        })
    }

    /// Execute a query and return matching results as JSON
    pub async fn search(&self, query_text: &str) -> TantivyResult<SearchResults> {
        let start = Instant::now();
        // 1) Prepare searcher & parser
        let reader = self.index.reader_builder().try_into()?;
        let searcher = reader.searcher();
        let query_parser =
            tantivy::query::QueryParser::for_index(&self.index, vec![self.fields.body]);

        let query = query_parser.parse_query(query_text)?;
        let top_docs = searcher.search(
            &query,
            &tantivy::collector::TopDocs::with_limit(100_000_000),
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
        let file_lines = self.lines_map.get(file_path)?;
        let total = file_lines.len();
        if line > total {
            return None;
        }

        let start = line.saturating_sub(1).saturating_sub(n);
        let end = (line - 1 + n).min(total - 1);
        let snippet = file_lines[start..=end].join("\n");
        Some((snippet, (start + 1, end + 1)))
    }
}
