//! DidYouMean 由来の距離計算を、copとCLIで同じ規則に保つ。

pub(crate) fn jaro_winkler(left: &str, right: &str) -> f64 {
    let (mut short, mut long): (Vec<_>, Vec<_>) = (left.chars().collect(), right.chars().collect());
    if short.len() > long.len() {
        std::mem::swap(&mut short, &mut long);
    }
    let radius = if long.len() > 3 {
        long.len() / 2 - 1
    } else {
        0
    };
    let mut short_matches = vec![false; short.len()];
    let mut long_matches = vec![false; long.len()];
    let mut count = 0;
    for (position, character) in short.iter().enumerate() {
        for candidate in position.saturating_sub(radius)..(position + radius + 1).min(long.len()) {
            if !long_matches[candidate] && *character == long[candidate] {
                short_matches[position] = true;
                long_matches[candidate] = true;
                count += 1;
                break;
            }
        }
    }
    if count == 0 {
        return 0.0;
    }
    let short_order = short
        .iter()
        .zip(short_matches)
        .filter_map(|(character, found)| found.then_some(character));
    let long_order = long
        .iter()
        .zip(long_matches)
        .filter_map(|(character, found)| found.then_some(character));
    let transpositions = short_order
        .zip(long_order)
        .filter(|(left, right)| left != right)
        .count()
        / 2;
    let matches = count as f64;
    let distance = (matches / short.len() as f64
        + matches / long.len() as f64
        + (matches - transpositions as f64) / matches)
        / 3.0;
    if distance > 0.7 {
        let prefix = left
            .chars()
            .zip(right.chars())
            .take(4)
            .take_while(|(left, right)| left == right)
            .count();
        distance + prefix as f64 * 0.1 * (1.0 - distance)
    } else {
        distance
    }
}

pub(crate) fn levenshtein(left: &str, right: &str) -> usize {
    let right: Vec<_> = right.chars().collect();
    let mut costs: Vec<_> = (0..=right.len()).collect();
    for (row, left_character) in left.chars().enumerate() {
        let mut diagonal = costs[0];
        costs[0] = row + 1;
        for (column, right_character) in right.iter().enumerate() {
            let previous = costs[column + 1];
            costs[column + 1] = (costs[column] + 1)
                .min(previous + 1)
                .min(diagonal + usize::from(left_character != *right_character));
            diagonal = previous;
        }
    }
    costs[right.len()]
}

pub(crate) fn similar_names<'a>(
    input: &str,
    dictionary: impl IntoIterator<Item = &'a str>,
) -> Vec<String> {
    // NameSimilarityはDidYouMean::SpellCheckerを使う。最初の一候補だけに切り詰めない。
    let normalized = input.to_lowercase().replace('@', "");
    let length = normalized.chars().count();
    let threshold = if length > 3 { 0.834 } else { 0.77 };
    let normalize = |name: &str| name.to_lowercase().replace('@', "");
    let mut words: Vec<_> = dictionary
        .into_iter()
        .filter(|word| *word != input && jaro_winkler(&normalize(word), &normalized) >= threshold)
        .collect();
    words.sort_by(|left, right| {
        jaro_winkler(left, &normalized).total_cmp(&jaro_winkler(right, &normalized))
    });
    words.reverse();
    let corrections: Vec<_> = words
        .iter()
        .filter(|word| levenshtein(&normalize(word), &normalized) <= length.div_ceil(4))
        .map(|word| (*word).to_owned())
        .collect();
    if !corrections.is_empty() {
        return corrections;
    }
    words
        .into_iter()
        .find(|word| {
            levenshtein(&normalize(word), &normalized) < length.min(normalize(word).chars().count())
        })
        .map(|word| vec![word.to_owned()])
        .unwrap_or_default()
}
