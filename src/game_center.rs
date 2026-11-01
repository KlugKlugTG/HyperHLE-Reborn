/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Host-side replacement for the online Game Center service.
//!
//! touchHLE cannot reach Apple's Game Center, so games that check
//! `[GKLocalPlayer localPlayer].isAuthenticated` normally give up on
//! achievements and leaderboards entirely. When `--game-center-authenticated`
//! is enabled the local player reports itself as signed in, and this module
//! provides the "cloud" it then talks to: a small on-disk database of
//! achievement progress and leaderboard scores, keyed by bundle identifier.
//!
//! The database is a tab-separated text file (`touchHLE_gamecenter.txt`) in
//! the user data directory, so it can be inspected, edited, backed up or
//! deleted by hand. The app picker's Game Center screen reads it through
//! [summary].
//!
//! This is a prototype: there is no player account, no signing, no anti-cheat
//! and no remote sync. Achievement percentages only move upwards, like on the
//! real service, and only a reset clears progress.

use crate::paths;
use crate::Environment;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// Name of the file the local Game Center database is stored in.
pub const STORE_FILE: &str = "touchHLE_gamecenter.txt";

/// Player identifier reported by `-[GKLocalPlayer playerID]` when the local
/// player is signed in. Shaped like a real Game Center identifier, which
/// starts with `G:`, so games that validate the prefix keep working.
pub const PLAYER_ID: &str = "G:touchHLE";

/// Alias reported by `-[GKLocalPlayer alias]` and `-displayName`.
pub const PLAYER_ALIAS: &str = "HyperHLE Player";

/// Comment block written at the top of the database file.
const FILE_HEADER: &str = "\
# touchHLE local Game Center data (prototype).
# Tab-separated records: app, achievement, score.
# Delete this file, or use Reset in the app picker's Game Center screen,
# to erase all local Game Center progress.
";

/// One achievement, as last reported by a game.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Achievement {
    /// The identifier the game passed to `GKAchievement`.
    pub identifier: String,
    /// Percentage complete, 0.0 to 100.0. Never decreases.
    pub percent: f64,
    /// True once [Achievement::percent] reached 100.0.
    pub completed: bool,
    /// Unix timestamp (seconds) of the most recent report.
    pub last_reported: u64,
}

impl Achievement {
    /// Human-readable name for the achievement.
    ///
    /// Apple serves achievement titles from the App Store, which is not
    /// reachable here, so the title is derived from the identifier the game
    /// reported to. Games that ship their own titles read them from their own
    /// resources, so this is only what the emulator can show.
    pub fn title(&self) -> String {
        humanize_identifier(&self.identifier)
    }
}

/// One leaderboard entry: the best score reported for a leaderboard.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Score {
    /// The leaderboard identifier the game reported to.
    pub leaderboard: String,
    /// Best value reported so far.
    pub value: i64,
    /// Text form of [Score::value] as the game last formatted it.
    pub formatted: String,
    /// Unix timestamp (seconds) of the most recent report.
    pub last_reported: u64,
}

impl Score {
    /// Human-readable name for the leaderboard, derived from its identifier.
    pub fn title(&self) -> String {
        humanize_identifier(&self.leaderboard)
    }
}

/// Everything recorded for one app.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AppRecord {
    pub bundle_id: String,
    /// `CFBundleDisplayName`, used as the heading in the viewer.
    pub display_name: String,
    pub achievements: BTreeMap<String, Achievement>,
    pub scores: BTreeMap<String, Score>,
}

impl AppRecord {
    /// Number of achievements that have been completed.
    pub fn completed_achievements(&self) -> usize {
        self.achievements
            .values()
            .filter(|achievement| achievement.completed)
            .count()
    }

    /// Average progress over all recorded achievements, 0.0 to 100.0.
    pub fn average_percent(&self) -> f64 {
        if self.achievements.is_empty() {
            return 0.0;
        }
        let total: f64 = self
            .achievements
            .values()
            .map(|achievement| achievement.percent)
            .sum();
        total / self.achievements.len() as f64
    }
}

/// Result of reporting achievement progress.
pub struct AchievementUpdate {
    /// The merged achievement, as now stored.
    pub achievement: Achievement,
    /// The report pushed the achievement to 100% for the first time.
    pub newly_completed: bool,
    /// The stored percentage went up because of this report.
    pub improved: bool,
}

/// Result of reporting a leaderboard score.
pub struct ScoreUpdate {
    /// The merged score, as now stored.
    pub score: Score,
    /// The report beat the previously stored best.
    pub new_best: bool,
}

/// The in-memory database. Guarded by [STORE] because GameKit calls can come
/// from any guest thread.
#[derive(Default)]
struct Store {
    apps: BTreeMap<String, AppRecord>,
    loaded: bool,
}

static STORE: LazyLock<Mutex<Store>> = LazyLock::new(|| Mutex::new(Store::default()));

/// Run `f` with the database loaded. The mutex is poisoned only if a previous
/// holder panicked, and the data is still usable then, so recover from that.
fn with_store<R>(f: impl FnOnce(&mut Store) -> R) -> R {
    let mut store = STORE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    store.ensure_loaded();
    f(&mut store)
}

/// Same as [with_store], but writes the database back to disk afterwards.
fn with_store_and_save<R>(f: impl FnOnce(&mut Store) -> R) -> R {
    let mut store = STORE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    store.ensure_loaded();
    let result = f(&mut store);
    store.save();
    result
}

/// Get or create the record for one bundle.
fn app_record<'a>(
    apps: &'a mut BTreeMap<String, AppRecord>,
    bundle_id: &str,
    display_name: &str,
) -> &'a mut AppRecord {
    let record = apps
        .entry(bundle_id.to_string())
        .or_insert_with(|| AppRecord {
            bundle_id: bundle_id.to_string(),
            display_name: display_name.to_string(),
            ..Default::default()
        });
    if record.display_name.is_empty() && !display_name.is_empty() {
        record.display_name = display_name.to_string();
    }
    record
}

impl Store {
    fn ensure_loaded(&mut self) {
        if self.loaded {
            return;
        }
        let path = store_path();
        match std::fs::read_to_string(&path) {
            Ok(text) => self.apps = Self::parse(&text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => log!(
                "Warning: couldn't read local Game Center data {}: {}",
                path.display(),
                error
            ),
        }
        self.loaded = true;
    }

    fn save(&self) {
        let path = store_path();
        if let Err(error) = std::fs::write(&path, self.to_text()) {
            log!(
                "Warning: couldn't save local Game Center data {}: {}",
                path.display(),
                error
            );
        }
    }

    fn to_text(&self) -> String {
        let mut text = String::from(FILE_HEADER);
        for app in self.apps.values() {
            text.push_str(&format!(
                "app\t{}\t{}\n",
                escape(&app.bundle_id),
                escape(&app.display_name)
            ));
            for achievement in app.achievements.values() {
                text.push_str(&format!(
                    "achievement\t{}\t{}\t{}\t{}\t{}\n",
                    escape(&app.bundle_id),
                    escape(&achievement.identifier),
                    format_percent(achievement.percent),
                    u8::from(achievement.completed),
                    achievement.last_reported,
                ));
            }
            for score in app.scores.values() {
                text.push_str(&format!(
                    "score\t{}\t{}\t{}\t{}\t{}\n",
                    escape(&app.bundle_id),
                    escape(&score.leaderboard),
                    score.value,
                    escape(&score.formatted),
                    score.last_reported,
                ));
            }
        }
        text
    }

    /// Lenient parser: unknown record types and short lines are skipped, so a
    /// hand-edited file can never stop the emulator from starting.
    fn parse(text: &str) -> BTreeMap<String, AppRecord> {
        let mut apps: BTreeMap<String, AppRecord> = BTreeMap::new();
        for line in text.lines() {
            let line = line.trim_end_matches('\r');
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let fields: Vec<&str> = line.split('\t').collect();
            match fields.as_slice() {
                ["app", bundle_id, display_name] => {
                    let bundle_id = unescape(bundle_id);
                    let display_name = unescape(display_name);
                    app_record(&mut apps, &bundle_id, &display_name);
                }
                ["achievement", bundle_id, identifier, percent, completed, timestamp] => {
                    let percent = percent.parse::<f64>().unwrap_or(0.0).clamp(0.0, 100.0);
                    let achievement = Achievement {
                        identifier: unescape(identifier),
                        percent,
                        completed: *completed == "1" || percent >= 100.0,
                        last_reported: timestamp.parse::<u64>().unwrap_or(0),
                    };
                    let bundle_id = unescape(bundle_id);
                    let record = app_record(&mut apps, &bundle_id, "");
                    record
                        .achievements
                        .insert(achievement.identifier.clone(), achievement);
                }
                ["score", bundle_id, leaderboard, value, formatted, timestamp] => {
                    let score = Score {
                        leaderboard: unescape(leaderboard),
                        value: value.parse::<i64>().unwrap_or(0),
                        formatted: unescape(formatted),
                        last_reported: timestamp.parse::<u64>().unwrap_or(0),
                    };
                    let bundle_id = unescape(bundle_id);
                    let record = app_record(&mut apps, &bundle_id, "");
                    record.scores.insert(score.leaderboard.clone(), score);
                }
                _ => (),
            }
        }
        apps
    }

    fn record_achievement(
        &mut self,
        bundle_id: &str,
        display_name: &str,
        identifier: &str,
        percent: f64,
        now: u64,
    ) -> AchievementUpdate {
        let percent = percent.clamp(0.0, 100.0);
        let record = app_record(&mut self.apps, bundle_id, display_name);
        let achievement =
            record
                .achievements
                .entry(identifier.to_string())
                .or_insert_with(|| Achievement {
                    identifier: identifier.to_string(),
                    percent: 0.0,
                    completed: false,
                    last_reported: now,
                });
        let previous_percent = achievement.percent;
        let was_completed = achievement.completed;
        // Game Center keeps the best percentage it was told about; a replay
        // that reports 0% again must not erase earlier progress.
        if percent > achievement.percent {
            achievement.percent = percent;
        }
        if achievement.percent >= 100.0 {
            achievement.completed = true;
        }
        achievement.last_reported = now;
        let achievement = achievement.clone();
        AchievementUpdate {
            newly_completed: achievement.completed && !was_completed,
            improved: achievement.percent > previous_percent,
            achievement,
        }
    }

    fn record_score(
        &mut self,
        bundle_id: &str,
        display_name: &str,
        leaderboard: &str,
        value: i64,
        formatted: &str,
        now: u64,
    ) -> ScoreUpdate {
        let record = app_record(&mut self.apps, bundle_id, display_name);
        // The first report for a leaderboard is always the best score so far,
        // which is why it cannot be merged with the `value > score.value`
        // check below: creating the entry already stores that value.
        let mut new_best = false;
        let score = record
            .scores
            .entry(leaderboard.to_string())
            .or_insert_with(|| {
                new_best = true;
                Score {
                    leaderboard: leaderboard.to_string(),
                    value,
                    formatted: formatted.to_string(),
                    last_reported: now,
                }
            });
        if value > score.value {
            new_best = true;
            score.value = value;
            score.formatted = formatted.to_string();
        }
        score.last_reported = now;
        let score = score.clone();
        ScoreUpdate { score, new_best }
    }
}

/// `f64` without the trailing `.0`, so the file stays readable by hand.
fn format_percent(percent: f64) -> String {
    if percent.fract() == 0.0 {
        format!("{}", percent as i64)
    } else {
        format!("{percent}")
    }
}

/// Turn an identifier into something a person can read.
///
/// Game Center identifiers are machine-oriented, e.g.
/// `com.studio.game.achievements.first_win`, `ACH_FIRST_WIN`, `firstWin` or
/// `level-3`. The reverse-DNS prefix and a leading generic part (`ach`,
/// `achievements`, `lb`, `leaderboard`) carry no information for the player,
/// so they are dropped, and what is left is split on punctuation and
/// camel-case boundaries and capitalised.
///
/// Capitalisation keeps words recognisable: a mixed-case word is left alone
/// (`iPhone`), an all-caps word stays an acronym when the identifier is mixed
/// case and the word is short (`earn_XP`), and otherwise words are title-cased
/// (`ACH_FIRST_WIN` becomes "First Win").
pub fn humanize_identifier(identifier: &str) -> String {
    let name = strip_generic_prefix(strip_reverse_dns_prefix(identifier));
    // An identifier that is entirely upper case is shouting, not an acronym:
    // title-casing its words reads much better.
    let identifier_is_all_caps = !identifier.chars().any(|character| character.is_lowercase());

    let characters: Vec<char> = name.chars().collect();
    let mut words: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut previous: Option<char> = None;
    for (index, &character) in characters.iter().enumerate() {
        if is_word_separator(character) {
            push_word(&mut words, &mut current, identifier_is_all_caps);
            previous = None;
            continue;
        }
        // Split `firstWin` and `level1` into separate words.
        let next_is_lowercase = characters
            .get(index + 1)
            .is_some_and(|next| next.is_lowercase());
        let single_letter_prefix = current.chars().count() == 1;
        let boundary = previous.is_some_and(|previous| {
            // `iPhone` and `iOS` are single words, so a hump right after one
            // letter is not a word break.
            let camel_hump = previous.is_lowercase()
                && character.is_uppercase()
                && !(next_is_lowercase && single_letter_prefix);
            let digit_boundary = (previous.is_alphabetic() && character.is_ascii_digit())
                || (previous.is_ascii_digit() && character.is_alphabetic());
            camel_hump || digit_boundary
        });
        if boundary {
            push_word(&mut words, &mut current, identifier_is_all_caps);
        }
        current.push(character);
        previous = Some(character);
    }
    push_word(&mut words, &mut current, identifier_is_all_caps);

    if words.is_empty() {
        // Nothing readable (e.g. an identifier of only separators): show the
        // original so the player can still tell entries apart.
        return identifier.trim().to_string();
    }
    words.join(" ")
}

/// Characters that separate words in an identifier.
fn is_word_separator(character: char) -> bool {
    matches!(character, '.' | '_' | '-' | ':' | '/' | '\\' | ' ')
}

/// Leading identifier components that only say what kind of record this is,
/// and so are useless as part of a title.
const GENERIC_PREFIXES: &[&str] = &["ach", "achievement", "achievements", "lb", "leaderboard"];

/// Drop a leading reverse-DNS prefix such as `com.studio.game` from an
/// identifier, always keeping the last component.
///
/// A component counts as part of the prefix only if it looks like a bundle
/// identifier component (lowercase ASCII letters, digits, `-` and `_`), and
/// the first component must be short, because it is conventionally a
/// top-level domain such as `com`. That keeps names like `Level.3.Boss` or
/// `game.first` intact.
fn strip_reverse_dns_prefix(identifier: &str) -> &str {
    let parts: Vec<&str> = identifier.split('.').collect();
    if parts.len() < 2 {
        return identifier;
    }
    let mut prefix_length = 0;
    let mut index = 0;
    while index + 1 < parts.len() {
        let part = parts[index];
        let looks_like_bundle_component = !part.is_empty()
            && part.chars().all(|character| {
                character.is_ascii_lowercase()
                    || character.is_ascii_digit()
                    || character == '-'
                    || character == '_'
            });
        if !looks_like_bundle_component || (index == 0 && part.len() > 4) {
            break;
        }
        prefix_length += part.len() + 1; // + the separating dot
        index += 1;
    }
    &identifier[prefix_length..]
}

/// Drop a leading `ach.`/`ach_`/`lb_`-style component, so that
/// `ach_first_win` and `achievements.first_win` both read "First Win".
fn strip_generic_prefix(identifier: &str) -> &str {
    let Some(separator) = identifier.find(is_word_separator) else {
        return identifier;
    };
    let head = identifier[..separator].to_lowercase();
    if !GENERIC_PREFIXES.contains(&head.as_str()) {
        return identifier;
    }
    // `separator` is a byte offset, so step over the whole separator
    // character (it may be multi-byte, e.g. a slash in a path-like id).
    let separator_length = identifier[separator..]
        .chars()
        .next()
        .map_or(0, char::len_utf8);
    let rest = &identifier[separator + separator_length..];
    if rest.is_empty() {
        return identifier;
    }
    // Recursively drop further generic parts (`achievements.ach.first_win`).
    strip_generic_prefix(rest)
}

/// Push a finished word into `words`, normalising its capitalisation first.
fn push_word(words: &mut Vec<String>, current: &mut String, identifier_is_all_caps: bool) {
    if current.is_empty() {
        return;
    }
    let word = std::mem::take(current);
    words.push(normalize_word(&word, identifier_is_all_caps));
}

/// Apply the capitalisation rules described on [humanize_identifier].
fn normalize_word(word: &str, identifier_is_all_caps: bool) -> String {
    let has_lowercase = word.chars().any(|character| character.is_lowercase());
    let has_uppercase = word.chars().any(|character| character.is_uppercase());
    let has_letters = word.chars().any(|character| character.is_alphabetic());
    if has_lowercase && has_uppercase {
        // Mixed case is a word the developer wrote deliberately, e.g. `iPhone`.
        return word.to_string();
    }
    if has_letters && !has_lowercase && !identifier_is_all_caps && word.chars().count() <= 4 {
        // A short all-caps word inside a mixed-case identifier is an acronym.
        return word.to_string();
    }
    let word = if has_lowercase {
        word.to_string()
    } else {
        word.to_lowercase()
    };
    let mut characters = word.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().collect::<String>() + characters.as_str(),
        None => String::new(),
    }
}

/// Escape the characters that would otherwise break the tab-separated format.
fn escape(field: &str) -> String {
    let mut escaped = String::with_capacity(field.len());
    for character in field.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '\t' => escaped.push_str("\\t"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            character => escaped.push(character),
        }
    }
    escaped
}

/// Reverse of [escape].
fn unescape(field: &str) -> String {
    let mut unescaped = String::with_capacity(field.len());
    let mut characters = field.chars();
    while let Some(character) = characters.next() {
        if character != '\\' {
            unescaped.push(character);
            continue;
        }
        match characters.next() {
            Some('t') => unescaped.push('\t'),
            Some('n') => unescaped.push('\n'),
            Some('r') => unescaped.push('\r'),
            // A lone trailing backslash, or an unknown escape, is kept as-is so
            // hand-edited files are not silently corrupted.
            Some(character) => unescaped.push(character),
            None => unescaped.push('\\'),
        }
    }
    unescaped
}

/// Current Unix time in seconds.
fn timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

/// Where the local Game Center database lives.
pub fn store_path() -> PathBuf {
    paths::user_data_base_path().join(STORE_FILE)
}

/// Bundle identifier and display name of the app that is currently running.
fn current_app(env: &Environment) -> (String, String) {
    (
        env.bundle.bundle_identifier().to_string(),
        env.bundle.display_name().to_string(),
    )
}

/// Record achievement progress reported by the running app.
pub fn report_achievement(env: &Environment, identifier: &str, percent: f64) -> AchievementUpdate {
    let (bundle_id, display_name) = current_app(env);
    with_store_and_save(|store| {
        store.record_achievement(&bundle_id, &display_name, identifier, percent, timestamp())
    })
}

/// Record a leaderboard score reported by the running app.
pub fn report_score(
    env: &Environment,
    leaderboard: &str,
    value: i64,
    formatted: &str,
) -> ScoreUpdate {
    let (bundle_id, display_name) = current_app(env);
    with_store_and_save(|store| {
        store.record_score(&bundle_id, &display_name, leaderboard, value, formatted, timestamp())
    })
}

/// Achievements recorded for the running app, in identifier order.
pub fn achievements_for_current_app(env: &Environment) -> Vec<Achievement> {
    let (bundle_id, _) = current_app(env);
    with_store(|store| {
        store
            .apps
            .get(&bundle_id)
            .map(|record| record.achievements.values().cloned().collect())
            .unwrap_or_default()
    })
}

/// Leaderboard entries recorded for the running app, in identifier order.
pub fn scores_for_current_app(env: &Environment) -> Vec<Score> {
    let (bundle_id, _) = current_app(env);
    with_store(|store| {
        store
            .apps
            .get(&bundle_id)
            .map(|record| record.scores.values().cloned().collect())
            .unwrap_or_default()
    })
}

/// Forget the achievements of the running app, like
/// `+[GKAchievement resetAchievementsWithCompletionHandler:]` does.
pub fn reset_current_app(env: &Environment) {
    let (bundle_id, _) = current_app(env);
    with_store_and_save(|store| {
        if let Some(record) = store.apps.get_mut(&bundle_id) {
            record.achievements.clear();
        }
    });
}

/// Forget every recorded app. Used by the app picker's Game Center screen.
pub fn reset_all() {
    with_store_and_save(|store| store.apps.clear());
}

/// Every recorded app, sorted for display. Apps without any data are left out.
pub fn summary() -> Vec<AppRecord> {
    with_store(|store| {
        let mut apps: Vec<AppRecord> = store
            .apps
            .values()
            .filter(|record| !record.achievements.is_empty() || !record.scores.is_empty())
            .cloned()
            .collect();
        apps.sort_by(|a, b| {
            a.display_name
                .to_lowercase()
                .cmp(&b.display_name.to_lowercase())
                .then_with(|| a.bundle_id.cmp(&b.bundle_id))
        });
        apps
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_become_readable_titles() {
        assert_eq!(
            humanize_identifier("com.studio.game.achievements.first_win"),
            "First Win"
        );
        // A leading `ach`/`achievements`/`lb` part says nothing to the player.
        assert_eq!(humanize_identifier("ach_first_win"), "First Win");
        assert_eq!(humanize_identifier("ACH_FIRST_WIN"), "First Win");
        assert_eq!(humanize_identifier("lb.best_time"), "Best Time");
        assert_eq!(humanize_identifier("ACH_LEVEL_3_DONE"), "Level 3 Done");
        assert_eq!(humanize_identifier("firstWin"), "First Win");
        assert_eq!(humanize_identifier("level-3"), "Level 3");
        assert_eq!(humanize_identifier("com.studio.game.ach.level1"), "Level 1");
        // A dotted name that is not a bundle prefix keeps all its parts.
        assert_eq!(humanize_identifier("Level.3.Boss"), "Level 3 Boss");
        // A short all-caps word inside a mixed-case identifier is an acronym.
        assert_eq!(humanize_identifier("earn_XP"), "Earn XP");
        // Mixed case is preserved as the developer wrote it.
        assert_eq!(humanize_identifier("buy_iPhone_case"), "Buy iPhone Case");
        // An identifier without a bundle prefix is used as-is.
        assert_eq!(humanize_identifier("total_score"), "Total Score");
        // Only generic parts: nothing to strip down to.
        assert_eq!(humanize_identifier("ach"), "Ach");
        // Nothing readable: fall back to the original.
        assert_eq!(humanize_identifier("___"), "___");
        assert_eq!(humanize_identifier(""), "");
    }

    #[test]
    fn titles_come_from_the_identifier() {
        let achievement = Achievement {
            identifier: "com.studio.game.ach.first_win".to_string(),
            ..Default::default()
        };
        assert_eq!(achievement.title(), "First Win");
        let score = Score {
            leaderboard: "com.studio.game.lb.best_time".to_string(),
            ..Default::default()
        };
        assert_eq!(score.title(), "Best Time");
    }

    #[test]
    fn escapes_and_unescapes_awkward_identifiers() {
        let original = "com.example.ach\twith\ttabs\\and\\slashes\nand newlines";
        let escaped = escape(original);
        assert!(!escaped.contains('\t'));
        assert!(!escaped.contains('\n'));
        assert_eq!(unescape(&escaped), original);
        assert_eq!(unescape("trailing\\"), "trailing\\");
    }

    #[test]
    fn achievement_progress_never_goes_backwards() {
        let mut store = Store::default();
        let first = store.record_achievement("com.example", "Example", "ach.one", 40.0, 100);
        assert!(first.improved);
        assert!(!first.newly_completed);
        assert_eq!(first.achievement.percent, 40.0);

        let replay = store.record_achievement("com.example", "Example", "ach.one", 0.0, 200);
        assert!(!replay.improved);
        assert_eq!(replay.achievement.percent, 40.0);
        assert_eq!(replay.achievement.last_reported, 200);

        let done = store.record_achievement("com.example", "Example", "ach.one", 100.0, 300);
        assert!(done.newly_completed);
        assert!(done.achievement.completed);

        let again = store.record_achievement("com.example", "Example", "ach.one", 100.0, 400);
        assert!(!again.newly_completed);
    }

    #[test]
    fn percentages_are_clamped() {
        let mut store = Store::default();
        let update = store.record_achievement("com.example", "Example", "ach.one", 250.0, 1);
        assert_eq!(update.achievement.percent, 100.0);
        assert!(update.achievement.completed);
        let update = store.record_achievement("com.example", "Example", "ach.two", -10.0, 1);
        assert_eq!(update.achievement.percent, 0.0);
    }

    #[test]
    fn only_the_best_score_is_kept() {
        let mut store = Store::default();
        let first = store.record_score("com.example", "Example", "board", 10, "10", 1);
        assert!(first.new_best);
        let worse = store.record_score("com.example", "Example", "board", 5, "5", 2);
        assert!(!worse.new_best);
        assert_eq!(worse.score.value, 10);
        assert_eq!(worse.score.last_reported, 2);
        let best = store.record_score("com.example", "Example", "board", 99, "99", 3);
        assert!(best.new_best);
        assert_eq!(best.score.formatted, "99");
    }

    #[test]
    fn database_round_trips_through_text() {
        let mut store = Store::default();
        store.record_achievement("com.example.game", "Example Game", "ach.first", 50.0, 1000);
        store.record_achievement("com.example.game", "Example Game", "ach.second", 100.0, 2000);
        store.record_score("com.example.game", "Example Game", "board.best", 42, "42", 3000);

        let text = store.to_text();
        let apps = Store::parse(&text);
        assert_eq!(apps.len(), 1);
        let record = &apps["com.example.game"];
        assert_eq!(record.display_name, "Example Game");
        assert_eq!(record.achievements.len(), 2);
        assert_eq!(record.achievements["ach.first"].percent, 50.0);
        assert!(record.achievements["ach.second"].completed);
        assert_eq!(record.completed_achievements(), 1);
        assert_eq!(record.average_percent(), 75.0);
        assert_eq!(record.scores["board.best"].value, 42);
        assert_eq!(record.scores["board.best"].formatted, "42");
        // Writing the parsed data back out must not lose anything.
        let mut reparsed = Store::default();
        reparsed.apps = apps;
        assert_eq!(reparsed.to_text(), text);
    }

    #[test]
    fn malformed_lines_are_ignored() {
        let apps = Store::parse(
            "# comment\n\
             \n\
             nonsense\n\
             app\ttoo-few-fields\n\
             achievement\tonly\ttwo\n\
             app\tcom.example\tExample\n\
             achievement\tcom.example\tach.one\tnot-a-number\t1\tnope\n\
             score\tcom.example\tboard\t7\t7\t5\n",
        );
        assert_eq!(apps.len(), 1);
        let record = &apps["com.example"];
        // The bad achievement is kept with fallback numbers rather than
        // dropped, because the line still tells us the identifier exists.
        assert_eq!(record.achievements["ach.one"].percent, 0.0);
        assert_eq!(record.scores["board"].value, 7);
    }
}
