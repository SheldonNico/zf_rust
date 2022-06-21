use std::{path::{PathBuf, Path}, io::BufRead, ascii::AsciiExt};
use std::cmp::Ordering;

#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub path: String,
    pub name: Option<String>,
    pub rank: f64,
    pub ranges: Vec<Range>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Range {
    pub start: usize,
    pub end: usize
}

impl Candidate {
    pub fn collect<S: BufRead>(content: S, delimiter: u8, plain: bool) -> Vec<Self> {
        Vec::from_iter(content.split(delimiter).filter_map(|item| {
            if item.as_ref().unwrap().len() == 0 { return None; }
            let path = String::from_utf8(item.unwrap()).expect("not utf8");
            let name = if !plain {
                Path::new(&path).file_name().map(|s| s.to_string_lossy().into_owned())
            } else {
                None
            };
            Some(Self {
                path,
                name,
                rank: 0.0,
                ranges: vec![],
            })

        }))
    }
}

#[inline]
fn has_upper(query: &str) -> bool {
    query.chars().any(|c| c.is_ascii_uppercase())
}

#[inline]
pub fn split_query(query: &str) -> Vec<&str> {
    query.split(" ").collect()
}

pub fn rank_candidates(
    candidates: Vec<Candidate>,
    query: &str,
    keep_order: bool,
) -> Vec<Candidate> {
    let smart_case = !has_upper(query);
    let mut ranked = vec![];
    if query.len() > 0 {
        let query_tokens = split_query(query);
        for mut candidate in candidates.into_iter() {
            candidate.ranges = vec![Default::default(); query_tokens.len()];
            if rank_candidate(&mut candidate, &query_tokens, smart_case) {
                ranked.push(candidate);
            }
        }
    }

    if !keep_order {
        ranked.sort_by(|a, b| {
            let o = a.rank.partial_cmp(&b.rank).unwrap_or(Ordering::Equal);
            if !o.is_eq() { return o; }

            let o = a.path.len().cmp(&b.path.len());
            if !o.is_eq() { return o; }

            let o = a.path.cmp(&b.path);
            o
        });
    }

    ranked
}

fn rank_candidate(candidate: &mut Candidate, query_tokens: &[&str], smart_case: bool) -> bool {
    candidate.rank = 0.0;
    for (token, range) in query_tokens.into_iter().zip(candidate.ranges.iter_mut()) {
        if let Some(r) = rank_token(
            candidate.path.as_bytes(),
            candidate.name.as_ref().map(|n| n.as_bytes()),
            range,
            token.as_bytes(),
            smart_case
        ) {
            candidate.rank += r;
        } else {
            return false;
        }
    }

    true
}

fn index_of(slice: &[u8], start_index: usize, value: u8) -> Option<usize> {
    let shift = slice.into_iter().skip(start_index).position(|&ch| (ch as char).to_ascii_lowercase() == (value as char))?;
    Some(start_index + shift)
}

fn index_of_case_sensitive(slice: &[u8], start_index: usize, value: u8) -> Option<usize> {
    let shift = slice.into_iter().skip(start_index).position(|&ch| ch == value)?;
    Some(start_index + shift)
}

struct IndexIterator<'a> {
    str: &'a [u8],
    chr: u8,
    index: usize,
    smart_case: bool
}

impl<'a> IndexIterator<'a> {
    pub fn new(str: &'a [u8], chr: u8, smart_case: bool) -> Self {
        Self { str, chr, index: 0, smart_case }
    }
}

impl<'a> Iterator for IndexIterator<'a> {
    type Item = usize;

    fn next(&mut self) -> Option<Self::Item> {
        let out = if self.smart_case {
            index_of(self.str, self.index, self.chr)
        } else {
            index_of_case_sensitive(self.str, self.index, self.chr)
        };
        if let Some(i) = out {
            self.index = i + 1;
        }
        out
    }
}

struct Match {
    rank: f64,
    start: usize,
    end: usize,
}

#[inline]
pub fn is_start_of_word(byte: u8) -> bool {
    matches!(byte as char, std::path::MAIN_SEPARATOR | '_' | '-' | '.' | ' ')
}

/// this is the core of the ranking algorithm. special precedence is given to
/// filenames. if a match is found on a filename the candidate is ranked higher
fn scan_to_end(name: &[u8], token: &[u8], start_index: usize, smart_case: bool) -> Option<Match> {
    let mut matched = Match { rank: 1.0, start: start_index, end: 0};
    let mut last_index = start_index;
    let mut last_sequential = false;

    // penalty for not starting on a word boundary
    if start_index > 0 && !is_start_of_word(name[start_index-1]) {
        matched.rank += 2.0;
    }

    for &chr in token.into_iter() {
        if let Some(index) = if smart_case {
            index_of(name, last_index+1, chr)
        } else {
            index_of_case_sensitive(name, last_index+1, chr)
        } {
            if index == last_index + 1 {
                // sequential matches only count the first character
                if !last_sequential {
                    last_sequential = true;
                    matched.rank += 1.0;
                }
            } else {
                // penalty for not starting on a word boundary
                if !is_start_of_word(name[index - 1]) {
                    matched.rank += 2.0;
                }

                last_sequential = false;
                matched.rank += (index - last_index) as f64;
            }

            last_index = index;
        } else {
            return None;
        }
    }

    matched.end = last_index;
    Some(matched)
}

fn rank_token(path: &[u8], name: Option<&[u8]>, range: &mut Range, token: &[u8], smart_case: bool) -> Option<f64> {
    // iterate over the indexes where the first char of the token matches
    use std::f64::MAX;
    let mut best_rank: f64 = MAX;
    if let Some(name) = name {
        let offs = path.len() - name.len();
        for start_index in IndexIterator::new(path, token[0], smart_case) {
            if let Some(matched) = scan_to_end(name, &token[1..], start_index, smart_case) {
                if best_rank == MAX || matched.rank < best_rank {
                    best_rank = matched.rank;
                    *range = Range {
                        start: matched.start + offs,
                        end: matched.end + offs,
                    };
                }
            } else {
                break;
            }
        }
    }

    if best_rank < MAX {
        best_rank = best_rank / 2.0;
        // how much of the token matched the filename?
        let token_len = token.len();
        let name_len = name.unwrap().len();
        if token_len == name_len {
            best_rank = best_rank / 2.0;
        } else {
            let coverage = 1.0 - (token_len as f64) / (name_len as f64);
            best_rank *= coverage;
        }
    } else {
        // retry on the full string
        for start_index in IndexIterator::new(path, token[0], smart_case) {
            if let Some(matched) = scan_to_end(path, &token[1..], start_index, smart_case) {
                if best_rank == MAX || matched.rank < best_rank {
                    best_rank = matched.rank;
                    *range = Range {
                        start: matched.start,
                        end: matched.end,
                    };
                }
            } else {
                break;
            }
        }

    }

    if best_rank == MAX { None } else { Some(best_rank) }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collect_candidates_whitespace() {
        let out = Candidate::collect("first second third fourth".as_bytes(), b' ', false);

        assert_eq!(out.len(), 4);
        assert_eq!(out[0].path.to_str().unwrap(), "first");
        assert_eq!(out[1].path.to_str().unwrap(), "second");
        assert_eq!(out[2].path.to_str().unwrap(), "third");
        assert_eq!(out[3].path.to_str().unwrap(), "fourth");
    }

    #[test]
    fn collect_candidates_newline() {
        let out = Candidate::collect("first\nsecond\nthird\nfourth".as_bytes(), b'\n', false);

        assert_eq!(out.len(), 4);
        assert_eq!(out[0].path.to_str().unwrap(), "first");
        assert_eq!(out[1].path.to_str().unwrap(), "second");
        assert_eq!(out[2].path.to_str().unwrap(), "third");
        assert_eq!(out[3].path.to_str().unwrap(), "fourth");
    }

    #[test]
    fn collect_candidates_excess_newline() {
        let out = Candidate::collect("first   second   third    fourth".as_bytes(), b' ', false);

        assert_eq!(out.len(), 4);
        assert_eq!(out[0].path.to_str().unwrap(), "first");
        assert_eq!(out[1].path.to_str().unwrap(), "second");
        assert_eq!(out[2].path.to_str().unwrap(), "third");
        assert_eq!(out[3].path.to_str().unwrap(), "fourth");
    }
}
