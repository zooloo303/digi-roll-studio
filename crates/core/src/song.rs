// Song mode: a list of rows, each naming a scene, played in order.
//
// PLAN.md §6 phase 12. Modelled on the DT2/DN2 SONG page, column for column,
// with one substitution that comes straight out of §2: **a row names a scene,
// not a pattern.** On the box a song row names one pattern on one box; here a
// scene already means *one pattern per box, chosen together*, so a row that
// names a scene moves the DT2 and the DN2 at the same boundary and needs no
// second pattern-resolution path. `Scheduler::commit_scene` is still the only
// thing in the app that moves a box onto a pattern.
//
// | The box's SONG page | Here |
// |---|---|
// | SONG ROW, 01–99 | position in [`Song::rows`], capped at [`MAX_ROWS`] |
// | LABEL | [`SongRow::label`], with [`LABELS`] as the keyword list |
// | PTN | [`SongRow::scene`] |
// | ROW PLAY COUNT | [`SongRow::repeats`] |
// | ROW LENGTH | [`SongRow::length_steps`], `None` meaning the scene's own cycle |
// | ROW TEMPO | not modelled — the session has one clock (§2). See below. |
// | SONG POINTER | published by the engine, drawn in the transport bar |
// | ROW MUTE | [`SongRow::muted_tracks`] |
// | END: LOOP/STOP | [`Song::end`] |
// | SONG SLOT and SONG NAME | [`Song::name`]; one song per session |
//
// **ROW TEMPO is deliberately absent, decided 2026-08-22.** §2's "tempo is per
// session" is not a style preference here, it is what the engine's timeline is
// built on: a deadline is `next_step × step_seconds(bpm)` from a single start
// instant, so moving `bpm` mid-run rescales the whole timeline retroactively.
// Per-row BPM therefore needs a piecewise tempo map through `engine::time`, the
// scheduler's cursor deadlines and clock counter, and the transport's
// elapsed→steps publish — a bigger job than the chaining, and one that would
// also fix the existing mid-play `SetTempo` rescale. The SONG panel shows the
// session tempo on every row as a *report*, so the column is visibly inherited
// rather than missing.
//
// **ROW LENGTH counts reference steps, not a pattern's steps.** The box has one
// length per pattern; this app has per-track `length_steps` and per-track
// `scale`, which is real polymeter (§2), so "play 32 steps of this row" has to
// name whose steps. It names none of them: a length is a count of 1/16 steps at
// 1x, session-wide, which is the same grid the transport's position readout and
// the MIDI clock already run on. `None` is not a number in that unit or any
// other — it means *the scene's own cycle*, which only the engine can answer
// because SCALE makes a step's length a per-track fact
// (`engine::scheduler::scene_cycle_seconds`). So an untouched row behaves
// exactly like today's boundary switch, and a shortened one cuts every track
// mid-pattern, which is what a fill row is.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::device::DeviceId;

/// The box's SONG ROW range is 01–99, and this is the same list.
pub const MAX_ROWS: usize = 99;

/// ROW LENGTH's range on the box, in this app's reference steps. The box writes
/// the last 25 values as K00–K24; nothing here needs that spelling, because
/// there is no encoder to turn.
pub const ROW_LENGTH_MIN: u16 = 2;
pub const ROW_LENGTH_MAX: u16 = 1024;

/// The LABEL keywords the box offers. A row's label is free text — it "can also
/// be the name of the pattern", so it was never a closed set — and these are
/// what the panel offers as one click each.
pub const LABELS: [&str; 10] = [
    "INTRO", "VERSE", "BRIDGE", "CHORUS", "FILL", "BREAK", "DROP", "OUTRO", "A", "B",
];

/// What happens after the last row — the box's END row, which is always there
/// and is not a row you can put a pattern on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EndAction {
    /// Back to row 1 and round again. The box's default, and so this one's.
    #[default]
    Loop,
    /// Stop the transport, releasing everything sounding.
    Stop,
}

impl EndAction {
    pub fn label(self) -> &'static str {
        match self {
            Self::Loop => "LOOP",
            Self::Stop => "STOP",
        }
    }
}

/// One row of the song.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SongRow {
    /// The box's LABEL: a keyword for what this row is in the arrangement.
    /// Free text, because the box lets it be a pattern name too.
    pub label: String,
    /// Index into `Session::scenes`. A row that names a scene the session does
    /// not have is a broken song and [`Song::validate_against`] says so rather
    /// than clamping — playing the wrong scene would hide the bug, which is the
    /// argument `Scheduler::queue_scene` already makes for the same case.
    pub scene: usize,
    /// ROW PLAY COUNT: how many times this row plays before the song moves on.
    /// Never zero — a row that plays no times is a row that should be deleted,
    /// and a zero here would be a boundary the engine waits on forever.
    pub repeats: u16,
    /// ROW LENGTH in reference steps (1/16 at 1x), or `None` for the scene's own
    /// cycle. See this file's header for why the unit is not a pattern's steps.
    pub length_steps: Option<u16>,
    /// ROW MUTE, as a bitmask per box: bit *n* set means track *n* is muted for
    /// this row.
    ///
    /// A device **absent from the map inherits the pattern's own mute state**,
    /// which is a third state and not the same as a mask of zero. The box does
    /// this too, from the other end: "when selecting a pattern for the row, the
    /// row's mute state initially reflects the pattern's mute state". Absent is
    /// that sentence with nothing stored; a mask of zero is a user who has
    /// unmuted everything on this row and means it.
    ///
    /// A mask rather than a `Vec<bool>` because the engine reads it per trig and
    /// no box in the table has more than 16 tracks. `BTreeMap` for the reason
    /// `Scene::slots` is one: saving an unchanged project twice must produce
    /// identical bytes.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub muted_tracks: BTreeMap<DeviceId, u32>,
}

impl SongRow {
    /// A row on `scene`, playing once, at the scene's own length, inheriting
    /// every box's mute state — which is the row the box adds when you press
    /// ADD, and the one every field of this struct defaults towards.
    pub fn new(scene: usize) -> Self {
        Self {
            label: String::new(),
            scene,
            repeats: 1,
            length_steps: None,
            muted_tracks: BTreeMap::new(),
        }
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    pub fn with_repeats(mut self, repeats: u16) -> Self {
        self.repeats = repeats.max(1);
        self
    }

    pub fn with_length(mut self, steps: Option<u16>) -> Self {
        self.length_steps = steps.map(clamp_row_length);
        self
    }

    /// ROW PLAY COUNT, never zero however the field got there — a song off disk
    /// with a zero in it would otherwise be a row the engine never leaves.
    pub fn plays(&self) -> u16 {
        self.repeats.max(1)
    }

    /// Whether track `track` of `device` is muted *by this row*. `None` when the
    /// row says nothing about that box, which means the pattern's own mute
    /// state stands.
    pub fn mutes(&self, device: DeviceId, track: usize) -> Option<bool> {
        let mask = self.muted_tracks.get(&device).copied()?;
        Some(track < 32 && mask & (1 << track) != 0)
    }

    /// Set the row's mute mask for one box, from the pattern's own mute flags —
    /// the box's "initially reflects the pattern's mute state".
    pub fn adopt_mutes(&mut self, device: DeviceId, muted: impl IntoIterator<Item = bool>) {
        let mut mask = 0u32;
        for (index, m) in muted.into_iter().enumerate().take(32) {
            if m {
                mask |= 1 << index;
            }
        }
        self.muted_tracks.insert(device, mask);
    }

    /// Mute or unmute one track for this row. Inserts a mask for the box if it
    /// had none — the row stops inheriting the moment it is edited, which is the
    /// only reading of a click on a mute button that does what it looks like.
    pub fn set_mute(&mut self, device: DeviceId, track: usize, muted: bool) {
        if track >= 32 {
            return;
        }
        let mask = self.muted_tracks.entry(device).or_insert(0);
        if muted {
            *mask |= 1 << track;
        } else {
            *mask &= !(1 << track);
        }
    }

    /// Give the box back its pattern mute state on this row.
    pub fn inherit_mutes(&mut self, device: DeviceId) {
        self.muted_tracks.remove(&device);
    }

    /// Whether this row overrides any box's mutes — what the panel's mute
    /// column lights for, matching the box's "a mute icon is displayed in the
    /// rows that have tracks that are muted".
    pub fn has_mutes(&self) -> bool {
        self.muted_tracks.values().any(|m| *m != 0)
    }
}

/// ROW LENGTH, held to the range the box's own encoder has.
pub fn clamp_row_length(steps: u16) -> u16 {
    steps.clamp(ROW_LENGTH_MIN, ROW_LENGTH_MAX)
}

/// Whether a track sounds, given the song row's mute mask and session-wide solo.
///
/// `row_mute` is [`SongRow::mutes`]: `Some` when the row has taken over the mute
/// state for that box, `None` when the pattern's own `mute` stands. That is the
/// whole of ROW MUTE's interaction with the desk — one substitution, not a
/// second mute stage — so a row can silence a track the pattern plays *and*
/// sound one the pattern mutes.
///
/// **Solo is not part of the substitution.** It is session-wide desk state
/// (PLAN.md §2), not arrangement state: soloing a track to listen to it must not
/// be undone by whichever row happens to come round next.
pub fn audible(row_mute: Option<bool>, track: &crate::model::Track, any_solo: bool) -> bool {
    if row_mute.unwrap_or(track.mute) {
        return false;
    }
    if any_solo {
        return track.solo;
    }
    true
}

/// A song: rows in order, and what to do at the end of them.
///
/// One per session, not a bank of slots. The box has sixteen; a session here is
/// already a file you open, so a second arrangement is a second file, and the
/// slot number a device sync will eventually need is a property of that
/// transfer rather than of the model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Song {
    pub name: String,
    pub rows: Vec<SongRow>,
    pub end: EndAction,
}

impl Default for Song {
    fn default() -> Self {
        Self::new("Song 1")
    }
}

impl Song {
    /// An empty song. Empty and not one-row: a song with a row in it is a claim
    /// about the arrangement, and the first row should be one somebody chose.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            rows: Vec::new(),
            end: EndAction::Loop,
        }
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn row(&self, index: usize) -> Option<&SongRow> {
        self.rows.get(index)
    }

    pub fn row_mut(&mut self, index: usize) -> Option<&mut SongRow> {
        self.rows.get_mut(index)
    }

    /// Append a row, up to [`MAX_ROWS`]. Returns its index, or `None` when the
    /// song is full — refused rather than silently dropped, because a row that
    /// does not appear is indistinguishable from a click that missed.
    pub fn push(&mut self, row: SongRow) -> Option<usize> {
        if self.rows.len() >= MAX_ROWS {
            return None;
        }
        self.rows.push(row);
        Some(self.rows.len() - 1)
    }

    /// Insert a copy of `index` directly after it — the box's duplicate, and the
    /// fastest way to build a song out of a verse that already exists.
    pub fn duplicate(&mut self, index: usize) -> Option<usize> {
        if self.rows.len() >= MAX_ROWS {
            return None;
        }
        let row = self.rows.get(index)?.clone();
        self.rows.insert(index + 1, row);
        Some(index + 1)
    }

    pub fn remove(&mut self, index: usize) -> Option<SongRow> {
        (index < self.rows.len()).then(|| self.rows.remove(index))
    }

    /// Move a row one place up or down. Returns where it ended up, or `None` if
    /// it could not move — the top row cannot go up.
    pub fn move_row(&mut self, index: usize, down: bool) -> Option<usize> {
        let to = if down { index + 1 } else { index.checked_sub(1)? };
        if index >= self.rows.len() || to >= self.rows.len() {
            return None;
        }
        self.rows.swap(index, to);
        Some(to)
    }

    /// Every row's scene index, deduplicated in first-play order — which scenes
    /// this song actually needs.
    pub fn scenes_used(&self) -> Vec<usize> {
        let mut out: Vec<usize> = Vec::new();
        for row in &self.rows {
            if !out.contains(&row.scene) {
                out.push(row.scene);
            }
        }
        out
    }

    /// Re-point rows after the scene *list* moved under them. Removing a scene
    /// shifts every index above it, exactly as it does for
    /// `Session::current_scene` and for the number the engine is holding.
    ///
    /// A row that named the removed scene lands on `replacement`, because the
    /// alternative is a row naming nothing and a song that cannot play through.
    /// Returns how many rows moved.
    pub fn scene_removed(&mut self, removed: usize, replacement: usize) -> usize {
        let mut moved = 0;
        for row in &mut self.rows {
            let was = row.scene;
            if row.scene == removed {
                row.scene = replacement;
            } else if row.scene > removed {
                row.scene -= 1;
            }
            if row.scene != was {
                moved += 1;
            }
        }
        moved
    }

    /// Drop a box's mute masks from every row — for a device leaving the
    /// session, so a re-added box does not inherit the mutes of the one it
    /// replaced.
    pub fn device_removed(&mut self, device: DeviceId) {
        for row in &mut self.rows {
            row.muted_tracks.remove(&device);
        }
    }

    /// The rows naming a scene this session does not have.
    ///
    /// Not a `Result`: a song is edited while the scene list is edited, so a
    /// transient broken row is normal and the panel marks it. What must never
    /// happen is the *engine* acting on one, and it does not —
    /// `commit_scene` ignores a scene index past the end.
    pub fn broken_rows(&self, num_scenes: usize) -> Vec<usize> {
        self.rows
            .iter()
            .enumerate()
            .filter(|(_, r)| r.scene >= num_scenes)
            .map(|(i, _)| i)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(n: u64) -> DeviceId {
        DeviceId(n)
    }

    #[test]
    fn a_new_row_inherits_everything() {
        let row = SongRow::new(0);
        assert_eq!(row.plays(), 1);
        assert_eq!(row.length_steps, None);
        // Absent, not zero: the pattern's own mutes stand.
        assert_eq!(row.mutes(device(1), 0), None);
        assert!(!row.has_mutes());
    }

    #[test]
    fn a_zero_play_count_off_disk_still_plays_once() {
        let row = SongRow {
            repeats: 0,
            ..SongRow::new(0)
        };
        assert_eq!(row.plays(), 1);
    }

    #[test]
    fn editing_one_mute_stops_the_row_inheriting() {
        let mut row = SongRow::new(0);
        row.set_mute(device(1), 3, true);
        assert_eq!(row.mutes(device(1), 3), Some(true));
        // The rest of that box is now explicitly unmuted, not inherited.
        assert_eq!(row.mutes(device(1), 4), Some(false));
        // The other box is untouched.
        assert_eq!(row.mutes(device(2), 3), None);

        row.inherit_mutes(device(1));
        assert_eq!(row.mutes(device(1), 3), None);
    }

    #[test]
    fn adopting_a_patterns_mutes_copies_the_flags() {
        let mut row = SongRow::new(0);
        row.adopt_mutes(device(1), [false, true, false, true]);
        assert_eq!(row.mutes(device(1), 0), Some(false));
        assert_eq!(row.mutes(device(1), 1), Some(true));
        assert_eq!(row.mutes(device(1), 3), Some(true));
        assert!(row.has_mutes());
    }

    #[test]
    fn a_row_of_zeroed_mutes_is_not_the_same_as_inheriting() {
        let mut row = SongRow::new(0);
        row.adopt_mutes(device(1), [false, false]);
        // Nothing is muted, but the row has taken over from the pattern.
        assert!(!row.has_mutes());
        assert_eq!(row.mutes(device(1), 0), Some(false));
    }

    #[test]
    fn row_length_is_held_to_the_boxs_range() {
        assert_eq!(SongRow::new(0).with_length(Some(0)).length_steps, Some(2));
        assert_eq!(SongRow::new(0).with_length(Some(9999)).length_steps, Some(1024));
        assert_eq!(SongRow::new(0).with_length(None).length_steps, None);
    }

    #[test]
    fn the_song_fills_up_at_ninety_nine_rows() {
        let mut song = Song::new("S");
        for _ in 0..MAX_ROWS {
            assert!(song.push(SongRow::new(0)).is_some());
        }
        assert_eq!(song.push(SongRow::new(0)), None);
        assert_eq!(song.len(), MAX_ROWS);
        assert_eq!(song.duplicate(0), None);
    }

    #[test]
    fn duplicate_lands_directly_after_its_source() {
        let mut song = Song::new("S");
        song.push(SongRow::new(0).with_label("INTRO"));
        song.push(SongRow::new(1).with_label("VERSE"));
        assert_eq!(song.duplicate(0), Some(1));
        assert_eq!(song.rows[1].label, "INTRO");
        assert_eq!(song.rows[2].label, "VERSE");
    }

    #[test]
    fn a_row_moves_and_the_ends_refuse_to() {
        let mut song = Song::new("S");
        song.push(SongRow::new(0).with_label("A"));
        song.push(SongRow::new(1).with_label("B"));
        assert_eq!(song.move_row(0, false), None);
        assert_eq!(song.move_row(1, true), None);
        assert_eq!(song.move_row(0, true), Some(1));
        assert_eq!(song.rows[0].label, "B");
    }

    #[test]
    fn removing_a_scene_shifts_the_rows_above_it() {
        let mut song = Song::new("S");
        song.push(SongRow::new(0));
        song.push(SongRow::new(1));
        song.push(SongRow::new(2));
        // Scene 1 goes; the row that named it lands on scene 0, and scene 2's row
        // follows the list down.
        assert_eq!(song.scene_removed(1, 0), 2);
        assert_eq!(
            song.rows.iter().map(|r| r.scene).collect::<Vec<_>>(),
            vec![0, 0, 1]
        );
    }

    #[test]
    fn scenes_used_is_first_play_order_without_repeats() {
        let mut song = Song::new("S");
        for scene in [2, 0, 2, 1, 0] {
            song.push(SongRow::new(scene));
        }
        assert_eq!(song.scenes_used(), vec![2, 0, 1]);
    }

    #[test]
    fn a_row_past_the_scene_list_is_reported_not_repaired() {
        let mut song = Song::new("S");
        song.push(SongRow::new(0));
        song.push(SongRow::new(7));
        assert_eq!(song.broken_rows(2), vec![1]);
        // Reported, and the row still says 7 — nothing has been clamped.
        assert_eq!(song.rows[1].scene, 7);
    }
}
