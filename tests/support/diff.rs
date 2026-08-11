//! 失敗時の差分表示。期待と実際の注記付きソースを行単位で突き合わせる。

/// 行単位の unified diff 風の表示を作る。`-` が期待のみ、`+` が実際のみ、
/// 空白が両方に在る行。テスト用ソースは数十行なので素朴な LCS で足りる。
pub fn unified(expected: &str, actual: &str) -> String {
    let left: Vec<&str> = expected.lines().collect();
    let right: Vec<&str> = actual.lines().collect();
    let common = longest_common_subsequence(&left, &right);

    let mut rendered = String::new();
    let mut left_index = 0;
    let mut right_index = 0;
    for (left_hit, right_hit) in common {
        while left_index < left_hit {
            rendered.push_str(&format!("-{}\n", left[left_index]));
            left_index += 1;
        }
        while right_index < right_hit {
            rendered.push_str(&format!("+{}\n", right[right_index]));
            right_index += 1;
        }
        rendered.push_str(&format!(" {}\n", left[left_hit]));
        left_index += 1;
        right_index += 1;
    }
    for line in &left[left_index..] {
        rendered.push_str(&format!("-{line}\n"));
    }
    for line in &right[right_index..] {
        rendered.push_str(&format!("+{line}\n"));
    }
    rendered
}

/// 一致する行の添字の組を、両側で昇順になるように返す。
fn longest_common_subsequence(left: &[&str], right: &[&str]) -> Vec<(usize, usize)> {
    let width = right.len() + 1;
    let mut lengths = vec![0usize; (left.len() + 1) * width];
    for left_index in (0..left.len()).rev() {
        for right_index in (0..right.len()).rev() {
            lengths[left_index * width + right_index] = if left[left_index] == right[right_index] {
                lengths[(left_index + 1) * width + right_index + 1] + 1
            } else {
                lengths[(left_index + 1) * width + right_index]
                    .max(lengths[left_index * width + right_index + 1])
            };
        }
    }

    let mut pairs = Vec::new();
    let (mut left_index, mut right_index) = (0, 0);
    while left_index < left.len() && right_index < right.len() {
        if left[left_index] == right[right_index] {
            pairs.push((left_index, right_index));
            left_index += 1;
            right_index += 1;
        } else if lengths[(left_index + 1) * width + right_index]
            >= lengths[left_index * width + right_index + 1]
        {
            left_index += 1;
        } else {
            right_index += 1;
        }
    }
    pairs
}
