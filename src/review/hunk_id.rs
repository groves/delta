use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

pub fn compute_hunk_id(file_path: &str, hunk_body_lines: &[String]) -> String {
    let mut hasher = DefaultHasher::new();
    file_path.hash(&mut hasher);
    for line in hunk_body_lines {
        line.hash(&mut hasher);
    }
    format!("{:016x}", hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_same_content_same_hash() {
        let lines = vec!["+added line".to_string(), " context".to_string()];
        let h1 = compute_hunk_id("src/main.rs", &lines);
        let h2 = compute_hunk_id("src/main.rs", &lines);
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_different_file_different_hash() {
        let lines = vec!["+added line".to_string()];
        let h1 = compute_hunk_id("src/main.rs", &lines);
        let h2 = compute_hunk_id("src/lib.rs", &lines);
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_different_content_different_hash() {
        let l1 = vec!["+line a".to_string()];
        let l2 = vec!["+line b".to_string()];
        let h1 = compute_hunk_id("file.rs", &l1);
        let h2 = compute_hunk_id("file.rs", &l2);
        assert_ne!(h1, h2);
    }
}
