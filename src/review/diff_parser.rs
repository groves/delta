use super::hunk_id;

pub struct FileDiff {
    pub path: String,
    pub hunks: Vec<HunkSegment>,
}

pub struct HunkSegment {
    pub plus_start: usize,
    pub content_hash: String,
    pub raw_segment: String,
}

pub fn parse_diff(diff_text: &str) -> Vec<FileDiff> {
    let lines: Vec<&str> = diff_text.lines().collect();
    let mut file_diffs = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        if lines[i].starts_with("diff --git ") {
            let (file_diff, next_i) = parse_file_diff(&lines, i);
            file_diffs.push(file_diff);
            i = next_i;
        } else {
            i += 1;
        }
    }

    file_diffs
}

fn parse_file_diff(lines: &[&str], start: usize) -> (FileDiff, usize) {
    let mut header_lines = Vec::new();
    let mut path = String::new();
    let mut i = start;

    // Collect header lines (diff --git through +++ line)
    while i < lines.len() {
        let line = lines[i];

        if line.starts_with("diff --git ") {
            header_lines.push(line.to_string());
            i += 1;
            continue;
        }

        if line.starts_with("--- ") {
            header_lines.push(line.to_string());
            i += 1;
            continue;
        }

        if line.starts_with("+++ ") {
            path = strip_diff_prefix(line, "+++ ");
            header_lines.push(line.to_string());
            i += 1;
            break;
        }

        if line.starts_with("@@") {
            // No --- / +++ lines (e.g. binary file or mode-only change)
            break;
        }

        if line.starts_with("diff --git ") && !header_lines.is_empty() {
            // Next file started without any hunks
            break;
        }

        // Other header lines (index, old mode, new mode, similarity, rename, etc.)
        header_lines.push(line.to_string());
        i += 1;
    }

    // If path is empty, try to extract from the diff --git line
    if path.is_empty()
        && let Some(first) = header_lines.first()
        && first.starts_with("diff --git ")
    {
        path = extract_path_from_diff_git(first);
    }

    // Parse hunks
    let mut hunks = Vec::new();
    while i < lines.len() {
        if lines[i].starts_with("diff --git ") {
            break;
        }

        if lines[i].starts_with("@@") {
            let (hunk, next_i) = parse_hunk(lines, i, &path, &header_lines);
            hunks.push(hunk);
            i = next_i;
        } else {
            i += 1;
        }
    }

    (FileDiff { path, hunks }, i)
}

fn parse_hunk(
    lines: &[&str],
    start: usize,
    file_path: &str,
    header_lines: &[String],
) -> (HunkSegment, usize) {
    let header = lines[start].to_string();
    let (plus_start, _plus_length) = parse_hunk_header_coords(&header);

    let mut hunk_lines = Vec::new();
    let mut i = start + 1;

    while i < lines.len() {
        let line = lines[i];
        if line.starts_with("diff --git ") || line.starts_with("@@") {
            break;
        }
        hunk_lines.push(line.to_string());
        i += 1;
    }

    let content_hash = hunk_id::compute_hunk_id(file_path, &hunk_lines);

    // Build the raw_segment: file headers + this single hunk
    let mut raw_segment = String::new();
    for h in header_lines {
        raw_segment.push_str(h);
        raw_segment.push('\n');
    }
    raw_segment.push_str(&header);
    raw_segment.push('\n');
    for line in &hunk_lines {
        raw_segment.push_str(line);
        raw_segment.push('\n');
    }

    (
        HunkSegment {
            plus_start,
            content_hash,
            raw_segment,
        },
        i,
    )
}

fn parse_hunk_header_coords(header: &str) -> (usize, usize) {
    // Parse @@ -x,y +a,b @@
    let plus_idx = header.find('+').unwrap_or(0);
    let rest = &header[plus_idx + 1..];
    let end = rest
        .find(' ')
        .or_else(|| rest.find('@'))
        .unwrap_or(rest.len());
    let coords = &rest[..end];

    let parts: Vec<&str> = coords.split(',').collect();
    let start = parts.first().and_then(|s| s.parse().ok()).unwrap_or(1);
    let length = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(1);

    (start, length)
}

fn strip_diff_prefix(line: &str, prefix: &str) -> String {
    let rest = &line[prefix.len()..];
    // Strip a/ or b/ prefix
    if rest.starts_with("a/") || rest.starts_with("b/") {
        rest[2..].to_string()
    } else {
        rest.to_string()
    }
}

fn extract_path_from_diff_git(line: &str) -> String {
    // "diff --git a/path b/path" -> "path"
    let rest = &line["diff --git ".len()..];
    if let Some(b_idx) = rest.rfind(" b/") {
        rest[b_idx + 3..].to_string()
    } else {
        rest.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_diff() {
        let diff = "\
diff --git a/src/main.rs b/src/main.rs
index abc..def 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,4 @@
 fn main() {
+    println!(\"hello\");
     println!(\"world\");
 }
";
        let result = parse_diff(diff);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].path, "src/main.rs");
        assert_eq!(result[0].hunks.len(), 1);
        assert_eq!(result[0].hunks[0].plus_start, 1);
        assert!(!result[0].hunks[0].content_hash.is_empty());
    }

    #[test]
    fn test_parse_multiple_hunks() {
        let diff = "\
diff --git a/file.rs b/file.rs
--- a/file.rs
+++ b/file.rs
@@ -1,3 +1,4 @@
 line1
+added
 line2
 line3
@@ -10,3 +11,2 @@
 line10
-removed
 line12
";
        let result = parse_diff(diff);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].hunks.len(), 2);
    }

    #[test]
    fn test_parse_hunk_header_coords() {
        assert_eq!(parse_hunk_header_coords("@@ -1,3 +1,4 @@"), (1, 4));
        assert_eq!(
            parse_hunk_header_coords("@@ -10,5 +20,8 @@ fn foo()"),
            (20, 8)
        );
    }
}
