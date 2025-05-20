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

use tantivy::schema::Value;
use tantivy::{
    doc,
    schema::{Field, Schema, STORED, TEXT},
    Index, Result as TantivyResult, TantivyDocument,
};
use walkdir::WalkDir;

use std::io::BufRead;

use std::time::Instant;

use std::fs;
use std::io;


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
}

impl CodeSearchEngine {
    /// Create a new search engine, build schema and index all files in directory
    pub fn new(dir: &str, memory: usize) -> TantivyResult<Self> {
        let mut schema_builder = Schema::builder();
        let path_field = schema_builder.add_text_field("path", STORED);
        let line_field = schema_builder.add_i64_field("line", STORED);
        let body_field = schema_builder.add_text_field("body", TEXT);
        let schema = schema_builder.build();

        let index = Index::create_in_ram(schema.clone());
        let mut writer = index.writer(memory)?;

        for entry in WalkDir::new(dir).into_iter().filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.is_file() {
                if let Some(name) = path.to_str() {
                    if name.contains(".git/") {
                        continue;
                    }
                }
                if let Ok(file) = fs::File::open(path) {
                    for (num, line) in io::BufReader::new(file).lines().enumerate() {
                        if let Ok(text) = line {
                            writer.add_document(doc!(
                                path_field => path.to_string_lossy().to_string(),
                                line_field => (num as i64 + 1),
                                body_field => text,
                            ))?;
                        }
                    }
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
        })
    }

    /// Execute a query and return matching results as JSON
    pub fn search(&self, query_text: &str) -> TantivyResult<SearchResults> {
        let start = Instant::now();
        // 1) Prepare searcher & parser
        let reader = self.index.reader_builder().try_into()?;
        let searcher = reader.searcher();
        let query_parser =
            tantivy::query::QueryParser::for_index(&self.index, vec![self.fields.body]);

        let query = query_parser.parse_query(query_text)?;

        let mut found_results: Vec<SearchResult> = Vec::new();

        let top_docs =
            searcher.search(&query, &tantivy::collector::TopDocs::with_limit(100000000))?;
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
                .unwrap();

            match Self::read_lines(file_path, line_num as usize, 3) {
                Ok((lines, (start, end))) => {
                    found_results.push(SearchResult {
                        body: lines,
                        path: file_path.to_string(),
                        line: line_num as usize,
                        line_range: LineRange{
                            start:start,
                            end: end,
                        }
                    });
                }
                Err(e) => {
                    println!("Error while reading {}: {}",file_path,  e);
                }
            }
        }
        let duration = start.elapsed();
        Ok(SearchResults {
            results: found_results,
            time: duration.as_secs_f64(),
        })
    }

    /// Helper method to read N lines around a target line from a file
    fn read_lines(file_path: &str, line: usize, n: usize) -> io::Result<(String, (usize, usize))> {
        let bytes = fs::read(file_path)?; // Read as raw bytes
        let text = String::from_utf8_lossy(&bytes); // Replace invalid UTF-8 with �
        let lines: Vec<&str> = text.split_inclusive('\n').collect();
        let total = lines.len();

        if line > total {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("line index {} out of range (0..{})", line, total),
            ));
        }

        let start = line.saturating_sub(n);
        let end = (line + n).min(total - 1);
        Ok((lines[start..=end].concat(), (start, end)))
    }
}
