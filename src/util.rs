fn div_ceil(a: u64, b: u64) -> u64 {
    if a == 0 {
        0
    } else {
        (a - 1) / b + 1
    }
}
fn english_pluralize(i: u64, singular: &str, plural: &str) -> String {
    if i == 1 {
        format!("1 {}", singular)
    } else {
        format!("{} {}", i, plural)
    }
}
pub fn to_english_time(secs: u64) -> String {
    if secs < 60 {
        english_pluralize(secs, "second", "seconds")
    } else if secs < 60 * 60 {
        english_pluralize(div_ceil(secs, 60), "minute", "minutes")
    } else {
        english_pluralize(div_ceil(secs, 60 * 60), "hour", "hours")
    }
}
