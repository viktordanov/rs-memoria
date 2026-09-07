pub fn search(corpus: &[String], term: &str) -> Vec<String> {
    corpus.iter().filter(|doc| doc.contains(term)).cloned().collect()
}
