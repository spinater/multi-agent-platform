//! Thai for Reflex. Reflex reads English words only, so Thai text is glossed before it is sent: split by longest match
//! against a glossary (Thai has no spaces between words) and each word replaced by its English. Local, deterministic
//! and LLM-free. The glossary is the built-in `templates/thai-glossary.tsv` plus the project's own words
//! (`settings.thai_glossary`). When too little of the Thai is known (`MIN_COVERAGE`), Reflex is not asked, as before.

use std::collections::{BTreeMap, HashMap};
use std::sync::LazyLock;

/// Share of the Thai characters the glossary must cover before the gloss is sent to Reflex.
pub const MIN_COVERAGE: f64 = 0.6;

const BUILTIN_TSV: &str = include_str!("../templates/thai-glossary.tsv");

pub fn is_thai(c: char) -> bool {
    ('\u{0E00}'..='\u{0E7F}').contains(&c)
}

pub fn has_thai(text: &str) -> bool {
    text.chars().any(is_thai)
}

/// Thai words and their English; an empty English is a word that carries no meaning (a particle).
pub struct Glossary {
    words: HashMap<String, String>,
    /// The longest Thai word, in chars: how far a match looks ahead.
    longest: usize,
}

/// Parses `<thai>\t<english>` lines; `#` comments and blank lines are skipped, `-` is an empty English.
pub fn parse_tsv(text: &str) -> Vec<(String, String)> {
    text.lines()
        .map(str::trim_end)
        .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
        .filter_map(|l| {
            let (th, en) = l.split_once('\t')?;
            let (th, en) = (th.trim(), en.trim());
            (!th.is_empty() && has_thai(th)).then(|| (th.to_string(), if en == "-" { String::new() } else { en.to_lowercase() }))
        })
        .collect()
}

static BUILTIN: LazyLock<Vec<(String, String)>> = LazyLock::new(|| parse_tsv(BUILTIN_TSV));

/// The number of built-in words.
pub fn builtin_len() -> usize {
    BUILTIN.len()
}

impl Glossary {
    /// The built-in words, then the project's (which win).
    pub fn new(extra: &BTreeMap<String, String>) -> Self {
        let mut words: HashMap<String, String> = BUILTIN.iter().cloned().collect();
        for (th, en) in extra {
            let th = th.trim();
            if has_thai(th) {
                let en = en.trim();
                words.insert(th.to_string(), if en == "-" { String::new() } else { en.to_lowercase() });
            }
        }
        let longest = words.keys().map(|k| k.chars().count()).max().unwrap_or(1);
        Self { words, longest }
    }
}

/// What Reflex reads for a text, and how much of its Thai the glossary knew.
#[derive(Debug, Clone, PartialEq)]
pub struct Gloss {
    /// The text with every known Thai word in English and the unknown Thai left out.
    pub text: String,
    pub thai_chars: usize,
    pub covered: usize,
    /// Thai runs no word matched, in order of appearance, without repeats.
    pub unknown: Vec<String>,
}

impl Gloss {
    pub fn coverage(&self) -> f64 {
        if self.thai_chars == 0 { 1.0 } else { self.covered as f64 / self.thai_chars as f64 }
    }

    /// Known well enough for Reflex.
    pub fn readable(&self) -> bool {
        self.coverage() >= MIN_COVERAGE && !self.text.trim().is_empty()
    }
}

fn thai_digit(c: char) -> Option<char> {
    ('๐'..='๙').contains(&c).then(|| char::from_u32('0' as u32 + (c as u32 - '๐' as u32)).unwrap_or(c))
}

/// Glosses `text`: longest match from left to right; non-Thai text (English words, code, numbers) is kept.
pub fn gloss(text: &str, g: &Glossary) -> Gloss {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut unknown: Vec<String> = vec![];
    let mut run = String::new();
    let (mut thai_chars, mut covered) = (0, 0);
    let push_word = |out: &mut String, w: &str| {
        if w.is_empty() {
            return;
        }
        if !out.is_empty() && !out.ends_with(char::is_whitespace) {
            out.push(' ');
        }
        out.push_str(w);
        out.push(' ');
    };
    let flush = |run: &mut String, unknown: &mut Vec<String>| {
        let r = std::mem::take(run);
        if r.chars().count() >= 2 && !unknown.contains(&r) {
            unknown.push(r);
        }
    };
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if !is_thai(c) {
            flush(&mut run, &mut unknown);
            out.push(c);
            i += 1;
            continue;
        }
        if let Some(d) = thai_digit(c) {
            flush(&mut run, &mut unknown);
            thai_chars += 1;
            covered += 1;
            out.push(d);
            i += 1;
            continue;
        }
        let max = g.longest.min(chars.len() - i);
        let hit = (1..=max).rev().find_map(|n| {
            let w: String = chars[i..i + n].iter().collect();
            g.words.get(&w).map(|en| (n, en))
        });
        match hit {
            Some((n, en)) => {
                flush(&mut run, &mut unknown);
                thai_chars += n;
                covered += n;
                push_word(&mut out, en);
                i += n;
            }
            None => {
                thai_chars += 1;
                run.push(c);
                i += 1;
            }
        }
    }
    flush(&mut run, &mut unknown);
    let text = out.split_whitespace().collect::<Vec<_>>().join(" ");
    Gloss { text, thai_chars, covered, unknown }
}

/// Character trigrams of `text` (lowercase, whitespace collapsed): a script-neutral fingerprint for near-duplicates.
fn trigrams(text: &str) -> std::collections::HashSet<[char; 3]> {
    let chars: Vec<char> = text.to_lowercase().split_whitespace().collect::<Vec<_>>().join(" ").chars().collect();
    chars.windows(3).map(|w| [w[0], w[1], w[2]]).collect()
}

/// Jaccard similarity of two texts' trigrams, 0..1; works for Thai and English alike.
pub fn similarity(a: &str, b: &str) -> f64 {
    let (x, y) = (trigrams(a), trigrams(b));
    if x.is_empty() || y.is_empty() {
        return 0.0;
    }
    let both = x.intersection(&y).count() as f64;
    both / ((x.len() + y.len()) as f64 - both)
}

pub const MAX_WORDS: usize = 3000;

pub fn preview(text: &str, extra: &BTreeMap<String, String>) -> crate::model::ThaiPreview {
    let g = gloss(text, &Glossary::new(extra));
    crate::model::ThaiPreview { readable: g.readable(), coverage: g.coverage(), has_thai: has_thai(text), unknown: g.unknown, text: g.text }
}

/// A project's words as saved: trimmed, Thai keys only, English (or `-`) required, within the limits.
pub fn clean_words(words: BTreeMap<String, String>) -> anyhow::Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for (th, en) in words {
        let (th, en) = (th.trim().to_string(), en.trim().to_string());
        if th.is_empty() && en.is_empty() {
            continue;
        }
        if !has_thai(&th) || th.chars().count() > 40 {
            anyhow::bail!("`{th}` is not a Thai word (up to 40 characters)");
        }
        if en.is_empty() || en.chars().count() > 80 || has_thai(&en) {
            anyhow::bail!("`{th}` needs its English (up to 80 characters, or - to skip the word)");
        }
        out.insert(th, en);
    }
    if out.len() > MAX_WORDS {
        anyhow::bail!("at most {MAX_WORDS} words");
    }
    Ok(out)
}

/// The project's words and what its recent Thai decisions did not cover.
pub async fn report(hub: &crate::hub::Hub, project: &crate::model::Project) -> anyhow::Result<crate::model::ThaiReport> {
    let g = Glossary::new(&project.settings.thai_glossary);
    let (mut thai, mut readable) = (0, 0);
    let mut counts: HashMap<String, u32> = HashMap::new();
    let mut seen_states = std::collections::HashSet::new();
    for d in hub.store.decisions(&project.slug, 500).await? {
        if !has_thai(&d.state) || !seen_states.insert(d.state.clone()) {
            continue;
        }
        thai += 1;
        let r = gloss(&d.state, &g);
        if r.readable() {
            readable += 1;
        }
        for u in r.unknown {
            *counts.entry(u).or_default() += 1;
        }
    }
    let mut unknown: Vec<crate::model::ThaiUnknown> = counts.into_iter().map(|(fragment, count)| crate::model::ThaiUnknown { fragment, count }).collect();
    unknown.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.fragment.cmp(&b.fragment)));
    unknown.truncate(60);
    Ok(crate::model::ThaiReport {
        builtin_words: builtin_len() as u32,
        min_coverage: MIN_COVERAGE,
        words: project.settings.thai_glossary.clone(),
        thai_decisions: thai,
        readable,
        unknown,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g() -> Glossary {
        Glossary::new(&BTreeMap::new())
    }

    #[test]
    fn the_builtin_glossary_parses() {
        assert!(builtin_len() > 600, "{}", builtin_len());
        assert!(BUILTIN.iter().all(|(th, _)| has_thai(th)));
    }

    #[test]
    fn glosses_a_ticket_by_longest_match() {
        let r = gloss("ลูกค้าโดนตัดเงินสองครั้งและต้องการเงินคืน", &Glossary::new(&BTreeMap::from([("โดน".into(), "-".into())])));
        assert_eq!(r.text, "customer charge twice want refund");
        assert!(r.readable());
        assert!(r.unknown.is_empty(), "{:?}", r.unknown);
    }

    #[test]
    fn keeps_english_and_code_and_reports_unknown_words() {
        let r = gloss("แก้บั๊ก login ใน src/auth.rs ให้ทำงานกับ กระต่ายวิเศษ", &g());
        assert!(r.text.starts_with("fix bug login in src/auth.rs"), "{}", r.text);
        assert!(r.text.contains("work with"), "{}", r.text);
        assert_eq!(r.unknown, vec!["กระต่ายวิเศษ".to_string()]);
        assert!(r.coverage() < 1.0);
    }

    #[test]
    fn mostly_unknown_thai_is_not_readable() {
        let r = gloss("ฉันชอบกินข้าวมันไก่ที่ร้านป้าแดงตรงหัวมุมถนน", &g());
        assert!(!r.readable(), "{} {}", r.coverage(), r.text);
    }

    #[test]
    fn project_words_win_and_thai_digits_become_numbers() {
        let extra = BTreeMap::from([("หน้าบ้าน".to_string(), "storefront".to_string()), ("กรุ๊ฟ".to_string(), "groove".to_string())]);
        let r = gloss("แก้หน้าบ้านของกรุ๊ฟ ๒ จุด", &Glossary::new(&extra));
        assert_eq!(r.text, "fix storefront groove 2", "{:?}", r);
    }

    #[test]
    fn saved_words_are_checked() {
        let ok = clean_words(BTreeMap::from([(" กรุ๊ฟ ".into(), "groove".into()), ("โดน".into(), "-".into()), (String::new(), String::new())])).unwrap();
        assert_eq!(ok, BTreeMap::from([("กรุ๊ฟ".into(), "groove".into()), ("โดน".into(), "-".into())]));
        assert!(clean_words(BTreeMap::from([("groove".into(), "groove".into())])).is_err());
        assert!(clean_words(BTreeMap::from([("กรุ๊ฟ".into(), String::new())])).is_err());
        assert!(clean_words(BTreeMap::from([("กรุ๊ฟ".into(), "กรุ๊ฟ".into())])).is_err());
    }

    #[test]
    fn english_only_text_is_unchanged() {
        let r = gloss("Fix the login bug", &g());
        assert_eq!(r.text, "Fix the login bug");
        assert_eq!(r.coverage(), 1.0);
    }

    #[test]
    fn similarity_finds_near_duplicates_in_thai_and_english() {
        assert!(similarity("เพิ่มปุ่มส่งออก CSV ในหน้ารายงาน", "เพิ่มปุ่มส่งออก CSV ในหน้ารายงานยอดขาย") > 0.7);
        assert!(similarity("Add a CSV export button to the reports page", "Add CSV export button on the reports page") > 0.6);
        assert!(similarity("เพิ่มปุ่มส่งออก CSV", "แก้บั๊กเข้าสู่ระบบ") < 0.2);
    }
}
