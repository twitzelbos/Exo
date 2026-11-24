//! The user's settings, one document for the account and one for this device. Each is JSON, in
//! its own row of the `settings` table. Unlike the rest of the database, it isn't a cache of Gmail.
//!
//! ```no_run
//! # async fn demo(store: &exo_store::Store) -> exo_store::Result<()> {
//! use exo_store::{Settings, Theme};
//!
//! let mut settings: Settings = store.settings().await?;
//! settings.theme = Theme::Carbon;
//! store.save_settings(&settings).await?;
//! # Ok(()) }
//! ```

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use toasty::Executor;

use crate::db::SettingsRow;
use crate::drafts::{Address, DraftAttachment};
use crate::error::Result;
use crate::out_of_office::{OutOfOfficeEventDetection, OutOfOfficeResponder};
use crate::split::{SplitEntry, SplitInbox, SplitInboxesV2, SystemSplit};
use crate::statistics::{Achievements, Statistics};
use crate::store::Store;

/// A settings document: [`Settings`] or [`DeviceSettings`].
pub trait Document: Serialize + DeserializeOwned + Default + sealed::Sealed {
    /// Its row in the `settings` table.
    const KEY: &'static str;
}

mod sealed {
    pub trait Sealed {}
    impl Sealed for super::Settings {}
    impl Sealed for super::DeviceSettings {}
}

/// The account's settings. A key that was never set takes its default: `theme` is Match System
/// and `unifiedInbox` is off (Important and Other on).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// `theme`.
    pub theme: Theme,
    /// `splitInboxesV2`: the splits, and their order among Important, Other and Inbox. Read and
    /// changed through [`Settings::split_entries`] and the methods after it.
    #[serde(skip_serializing_if = "SplitInboxesV2::is_unset")]
    split_inboxes_v2: SplitInboxesV2,
    /// `unifiedInbox`: one Inbox instead of Important and Other. Off for a new account, which has
    /// Important and Other.
    pub unified_inbox: bool,
    /// `splitsEngagement`.
    pub splits_engagement: SplitsEngagement,
    /// `searchHistory`: recent searches, oldest first.
    pub search_history: Vec<SearchHistoryItem>,
    /// `nudges`: when each nudge was first shown and last done with, keyed by the nudge's id.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub nudges: BTreeMap<String, Nudge>,
    /// `labelsLastUsed`: when each label was last added, or moved to, from the Label and Move
    /// pickers, as ISO timestamps keyed by label id. The pickers list recently used labels first.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub labels_last_used: BTreeMap<String, String>,
    /// `blockedSenders`: the addresses and domains blocked, oldest first. Gmail filters do the
    /// blocking, and this is the list Exo shows.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub blocked_senders: Vec<String>,
    /// `calendar`: the calendar's options.
    #[serde(skip_serializing_if = "CalendarSettings::is_default")]
    pub calendar: CalendarSettings,
    /// `calendarAccounts`: per account, which of its calendars sync and show, and their
    /// colours.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub calendar_accounts: BTreeMap<String, CalendarAccount>,
    /// `calendarInviteHint/{email}`: whether the day view's `Add Calendar` hint was answered
    /// (`No thanks` or `Sign in`) for an invite to `email`, a signed-out calendar account.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub calendar_invite_hint: BTreeMap<String, bool>,
    /// `meetingLinks`: the link new events get.
    #[serde(skip_serializing_if = "MeetingLinks::is_default")]
    pub meeting_links: MeetingLinks,
    /// `snippets`: the personal snippets.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub snippets: Vec<Snippet>,
    /// `snippetsLastUsed`: when each snippet was last used or saved, as ISO timestamps keyed by
    /// snippet id. The pickers and the Snippets folder rank snippets by it.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub snippets_last_used: BTreeMap<String, String>,
    /// `useSnippetHintCount`: how many times saving a snippet has shown `To use a Snippet, hit
    /// ⌘ ; at any time!` (three at most).
    #[serde(skip_serializing_if = "is_zero")]
    pub use_snippet_hint_count: u32,
    /// `disableNotifications`: email notifications off for the account, on every device. Exo
    /// only ever clears it. Turning notifications off writes the device's setting
    /// ([`DeviceSettings::disable_notifications`]), and turning them on clears both.
    #[serde(skip_serializing_if = "is_false")]
    pub disable_notifications: bool,
    /// `notifications`: when calendar notifications come.
    #[serde(skip_serializing_if = "Notifications::is_default")]
    pub notifications: Notifications,
    /// `previousSchedule`: the Send later time picked last, which the picker offers as `last
    /// used` while it's still ahead.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_schedule: Option<PreviousSchedule>,
    /// `remoteContentBlocking`: which remote images load in message bodies (the Images dialog).
    /// `None` until one is chosen. `None` reads as the default
    /// ([`RemoteContentBlocking::default`]), not as `none`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_content_blocking: Option<RemoteContentBlocking>,
    /// `reminders`: the Reminders dialog's defaults for Remind me.
    #[serde(skip_serializing_if = "ReminderSettings::is_default")]
    pub reminders: ReminderSettings,
    /// `previousReminder`: the last time chosen in Remind me, shown as Remind me's `last used`
    /// row.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_reminder: Option<PreviousReminder>,
    /// `hasShownFollowUpReminderHint`: the `Follow up on time, every time` alert has shown.
    #[serde(skip_serializing_if = "is_false")]
    pub has_shown_follow_up_reminder_hint: bool,
    /// `outOfOfficeResponder`: the copy of Gmail's vacation responder as last read or saved.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub out_of_office_responder: Option<OutOfOfficeResponder>,
    /// `outOfOfficeEventDetectionV2`: the out-of-office events the nudge offers a reply for.
    #[serde(
        rename = "outOfOfficeEventDetectionV2",
        skip_serializing_if = "is_default"
    )]
    pub out_of_office_event_detection: OutOfOfficeEventDetection,
    /// `desktopAutoAdvance`: where triaging a conversation goes next (the Auto Advance dialog).
    #[serde(skip_serializing_if = "is_default")]
    pub desktop_auto_advance: AutoAdvance,
    /// `sendAndDone`: sending a reply marks its conversation done (`Send + Mark Done`).
    #[serde(skip_serializing_if = "is_false")]
    pub send_and_done: bool,
    /// `rsvpAndDone`: answering an invitation marks its conversation done (`RSVP + Mark Done`).
    #[serde(skip_serializing_if = "is_false")]
    pub rsvp_and_done: bool,
    /// `showSenderFullNames`: message headers name the sender in full, not by first name.
    #[serde(skip_serializing_if = "is_false")]
    pub show_sender_full_names: bool,
    /// `includeSignatureOnReplies`: replies and forwards carry the From alias's signature.
    /// Unset reads as on ([`Settings::include_signature_on_replies`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_signature_on_replies: Option<bool>,
    /// `convertHyphenToBulletList`: `-` and Space at the start of a line start a bulleted list.
    #[serde(skip_serializing_if = "is_false")]
    pub convert_hyphen_to_bullet_list: bool,
    /// `enableBacktickAsEscape`: a backtick is Escape in compose too, as it is everywhere else.
    /// Unset until it's turned on or off, which the Touch Bar's tip asks about.
    /// [`Settings::backtick_as_escape`] reads unset as off.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_backtick_as_escape: Option<bool>,
    /// `customInstantIntro`: Instant Intro's thank-you line, empty for the default
    /// ([`Settings::instant_intro`]).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub custom_instant_intro: String,
    /// `emojiSkinTone`: the skin tone emoji are inserted with. Unset until the Emoji dialog
    /// opens or the first emoji that has tones is inserted. Either one sets it to `NONE`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emoji_skin_tone: Option<EmojiSkinTone>,
    /// `autoBcc`: what Auto Bcc adds to every draft's Bcc, as typed (`123@bcc.hubspot.com`,
    /// `CRM <123@bcc.hubspot.com>`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub auto_bcc: Vec<String>,
    /// `autoBccExclusions`: addresses and domains whose mail goes without Auto Bcc.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub auto_bcc_exclusions: Vec<String>,
    /// `excludedContacts`: addresses taken out of compose's suggestions (`Remove Suggestion`),
    /// each with the time it was taken out (ISO 8601).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub excluded_contacts: BTreeMap<String, String>,
    /// `quickTips`: whether the Quick Tips bar shows. Off until it's turned on.
    #[serde(skip_serializing_if = "is_default")]
    pub quick_tips: QuickTips,
    /// `hasShownCommandNudge`: the palette's first-run nudge has been seen, or a triage key has
    /// been used, so the nudge doesn't show again.
    #[serde(skip_serializing_if = "is_false")]
    pub has_shown_command_nudge: bool,
    /// `hasShownShareAvailabilityTooltip`: Share Availability's `You can now hit ⌘ shift A…`
    /// tooltip was dismissed.
    #[serde(skip_serializing_if = "is_false")]
    pub has_shown_share_availability_tooltip: bool,
    /// `doShowTutorial`: the first-run flow is still to run. Exo sets it with the welcome splash,
    /// the first time the account opens on this Mac, and the flow clears it as it starts.
    #[serde(skip_serializing_if = "is_false")]
    pub do_show_tutorial: bool,
    /// `doShowUndoSendFtue`: the Undo Send alert (`Congratulations 🙌`) is still to show on a
    /// send. Exo sets it with the tutorial, and dismissing the alert clears it.
    #[serde(skip_serializing_if = "is_false")]
    pub do_show_undo_send_ftue: bool,
    /// `showFirstTimeGMTZ`: Get Me To Zero is still to welcome the user to their inbox
    /// (`Welcome to your Inbox 👋`). Exo sets it as the first run ends in Get Me To Zero, and the
    /// next Get Me To Zero to close clears it, wherever it was opened from.
    #[serde(rename = "showFirstTimeGMTZ", skip_serializing_if = "is_false")]
    pub show_first_time_gmtz: bool,
    /// `typingAssistance`: autocorrect and autocomplete.
    #[serde(skip_serializing_if = "is_default")]
    pub typing_assistance: TypingAssistance,
    /// `statistics`: what Achievements count.
    #[serde(skip_serializing_if = "Statistics::is_empty")]
    pub statistics: Statistics,
    /// `reduceMotion`: Inbox Zero shows a moving photo's still (`Inbox Zero: Disable Animated
    /// Images`).
    #[serde(skip_serializing_if = "is_false")]
    pub reduce_motion: bool,
}

/// `quickTips`: the Quick Tips bar.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct QuickTips {
    /// `show`.
    pub show: bool,
}

/// `typingAssistance`: autocorrect and autocomplete. It keeps no languages or learned spellings,
/// because macOS's spellchecker detects the language and keeps the user's dictionary itself.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TypingAssistance {
    /// `enabled`: autocorrect. Until it's turned on or off in Exo, it's unset and follows macOS's
    /// `Correct spelling automatically` ([`Settings::autocorrect`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// `autocompleteEnabled`: unset reads as on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub autocomplete_enabled: Option<bool>,
}

/// `desktopAutoAdvance`: where the next conversation comes from once one leaves the list.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AutoAdvance {
    /// `down`: the older conversation, below.
    #[default]
    Down,
    /// `up`: the newer conversation, above.
    Up,
    /// `off`: back to the list.
    Off,
}

impl AutoAdvance {
    /// The step to the next conversation: 1 down, -1 up, 0 back to the list.
    pub fn direction(self) -> isize {
        match self {
            AutoAdvance::Down => 1,
            AutoAdvance::Up => -1,
            AutoAdvance::Off => 0,
        }
    }
}

/// `emojiSkinTone`: the skin tone emoji are inserted with.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EmojiSkinTone {
    /// The yellow default.
    #[default]
    None,
    Light,
    MediumLight,
    Medium,
    MediumDark,
    Dark,
}

impl EmojiSkinTone {
    /// The tones in the Emoji dialog's order.
    pub const ALL: [Self; 6] = [
        Self::None,
        Self::Light,
        Self::MediumLight,
        Self::Medium,
        Self::MediumDark,
        Self::Dark,
    ];
}

fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

/// `notifications`: when calendar notifications come.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Notifications {
    /// `calendarEvents`.
    #[serde(skip_serializing_if = "CalendarEventNotifications::is_default")]
    pub calendar_events: CalendarEventNotifications,
}

impl Notifications {
    fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// `notifications/calendarEvents`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CalendarEventNotifications {
    /// `leadTimeInSeconds`: how long before an event its notification comes, when the event
    /// keeps its calendar's default reminders. Two minutes while unset
    /// ([`CalendarEventNotifications::lead_time`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lead_time_in_seconds: Option<i64>,
}

impl CalendarEventNotifications {
    /// The lead time while none is set: two minutes.
    pub const DEFAULT_LEAD_TIME: i64 = 120;

    fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// `leadTimeInSeconds`, or its default.
    pub fn lead_time(&self) -> i64 {
        self.lead_time_in_seconds.unwrap_or(Self::DEFAULT_LEAD_TIME)
    }
}

/// `previousSchedule`: a Send later row as picked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreviousSchedule {
    /// The row's title (`tomorrow morning`, `in 3 days`).
    pub title: String,
    /// Its time.
    pub time: jiff::Timestamp,
}

/// `remoteContentBlocking`: which remote images message bodies load.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RemoteContentBlocking {
    /// `none`: `Show all images`, read receipts included.
    None,
    /// `reduce-tracking`: `Block all known tracking pixels`. This only catches a hardcoded tracker
    /// list and tiny pixels, and only inside `<img>` — a CSS `background-image` or SVG tracker
    /// still loads, and any normal remote image still reports your IP and open time to the sender,
    /// since there's no image proxy.
    ReduceTracking,
    /// `all`: `Block all remote images`, until the images in a message are shown. The default: with
    /// no image proxy, loading remote content reveals your IP and open time to the sender, so Exo
    /// blocks it until you choose to show a message's images.
    #[default]
    All,
}

/// `reminders`: when a reminder set without a time comes back, and what a new one does on a
/// reply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ReminderSettings {
    /// `days`: the number of days in Remind me's `in {n} days` for a reminder.
    pub days: u32,
    /// `hour` and `minute`: the time of day a reminder set without a time comes back at.
    pub hour: i8,
    pub minute: i8,
    /// `triggerCondition`: what the picker's dropdown starts on.
    pub trigger_condition: TriggerCondition,
    /// `defaultRemindersTrigger`: `NONE`, or `EXTERNAL` for a reminder on every draft with an
    /// external recipient.
    pub default_reminders_trigger: String,
    /// `businessDaysOnly`: only weekdays count as days. Unset until the Reminders dialog first
    /// opens and writes its default, which is on ([`ReminderSettings::fill_defaults`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub business_days_only: Option<bool>,
}

impl Default for ReminderSettings {
    /// In 2 days at 8:00 AM if no reply, with `businessDaysOnly` unset.
    fn default() -> Self {
        Self {
            days: 2,
            hour: 8,
            minute: 0,
            trigger_condition: TriggerCondition::IfNoReply,
            default_reminders_trigger: "NONE".into(),
            business_days_only: None,
        }
    }
}

impl ReminderSettings {
    fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Whether only weekdays count as days (`businessDaysOnly`), off while unset.
    #[must_use]
    pub fn weekdays_only(&self) -> bool {
        self.business_days_only.unwrap_or(false)
    }

    /// What the Reminders dialog writes as it opens while `reminders` is unset: the defaults,
    /// with `businessDaysOnly` on. An unset `reminders` reads as the defaults without
    /// `businessDaysOnly`, because Exo stores no object that is at its defaults. Returns whether
    /// that changed anything.
    pub fn fill_defaults(&mut self) -> bool {
        if *self != Self::default() {
            return false;
        }
        self.business_days_only = Some(true);
        true
    }
}

/// `triggerCondition`: whether a reply cancels a reminder.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TriggerCondition {
    /// `if no reply`: a reply cancels it.
    #[default]
    IfNoReply,
    /// `regardless`.
    Regardless,
}

/// `previousReminder`: `{ suggestion: { title, time }, dropdown: { value } }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviousReminder {
    /// The row chosen.
    pub suggestion: PreviousSuggestion,
    /// The dropdown's value then: `KEEP_ON_REPLY` or `IF_NO_REPLY`.
    pub dropdown: PreviousDropdown,
}

/// `previousReminder.suggestion`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreviousSuggestion {
    /// Its title: `tomorrow`, `in 3 days`.
    pub title: String,
    /// When it was for.
    pub time: jiff::Timestamp,
}

/// `previousReminder.dropdown`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreviousDropdown {
    /// `KEEP_ON_REPLY` (regardless) or `IF_NO_REPLY`.
    pub value: String,
}

/// `meetingLinks`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MeetingLinks {
    /// `defaultMeetingLink`: `google` (Meet for Google events), `zoom`, or none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_meeting_link: Option<String>,
}

impl MeetingLinks {
    fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

fn is_zero(count: &u32) -> bool {
    *count == 0
}

fn is_false(value: &bool) -> bool {
    !value
}

/// A personal snippet, which compose inserts into a draft.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Snippet {
    /// Its id, a draft id (`draft00…`).
    pub id: String,
    /// `name`, which the pickers and the folder show (the editor's `Enter description`).
    pub name: String,
    /// `subject`, which a new message without a subject takes.
    pub subject: String,
    /// `body`, HTML.
    pub body: String,
    /// `to`, added to the draft it's used in.
    pub to: Vec<Address>,
    /// `cc`.
    pub cc: Vec<Address>,
    /// `bcc`.
    pub bcc: Vec<Address>,
    /// `from`: the alias it switches a draft to, or `None` to keep the draft's own
    /// (`"use default"`).
    pub from: Option<Address>,
    /// Its files, kept with the drafts' files under their `uuid` ([`Store::draft_files`]).
    pub attachments: Vec<DraftAttachment>,
    /// `date`: when it was last saved, as an ISO timestamp.
    pub date: String,
    /// How it has been used ([`SnippetAnalytics`]). Saving the snippet keeps these counts.
    #[serde(flatten)]
    pub analytics: SnippetAnalytics,
}

/// How a snippet has been used, as Exo counts it from the messages it sends. There are no opens,
/// because counting them needs read statuses, which Exo hasn't got.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SnippetAnalytics {
    /// `sends`: messages sent with it.
    pub sends: u32,
    /// `replies`: the sent messages that someone else has replied to since.
    pub replies: u32,
    /// When a message was last sent with it, as an ISO timestamp (empty if never).
    pub last_sent: String,
    /// The sends that Exo hasn't seen a reply to yet, which it keeps looking at.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unreplied: Vec<SnippetSend>,
}

/// A message sent with a snippet: its conversation and Gmail's id for it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnippetSend {
    /// The conversation.
    pub thread: String,
    /// The sent message.
    pub message: String,
}

/// `calendar`: the calendar's options.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CalendarSettings {
    /// `weekStartsOn`: 0 for Sunday, 1 for Monday.
    pub week_starts_on: u8,
    /// `showWeekends`.
    pub show_weekends: bool,
    /// `showDeclinedEvents`.
    pub show_declined_events: bool,
    /// `showAdditionalTimezones`.
    pub show_additional_timezones: bool,
    /// `additionalTimezones`.
    pub additional_timezones: Vec<AdditionalTimezone>,
    /// `hasDismissedFindTimeEnterShortcutTooltip`: Meet's `Hit enter to instantly find times!`
    /// tooltip was closed.
    pub has_dismissed_find_time_enter_shortcut_tooltip: bool,
}

impl CalendarSettings {
    fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

impl Default for CalendarSettings {
    fn default() -> Self {
        Self {
            week_starts_on: 0,
            show_weekends: true,
            show_declined_events: false,
            show_additional_timezones: false,
            additional_timezones: Vec::new(),
            has_dismissed_find_time_enter_shortcut_tooltip: false,
        }
    }
}

/// A secondary time zone in the week view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdditionalTimezone {
    /// `ianaId`.
    #[serde(rename = "ianaId")]
    pub iana_id: String,
}

/// `calendarAccounts/{email}`: a Google account whose calendars Exo shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CalendarAccount {
    /// `email`.
    pub email: String,
    /// `authed`: signed in.
    pub authed: bool,
    /// `provider`: `google`.
    pub provider: String,
    /// `calendars`, by calendar id.
    pub calendars: BTreeMap<String, CalendarSetting>,
}

impl Default for CalendarAccount {
    fn default() -> Self {
        Self {
            email: String::new(),
            authed: true,
            provider: "google".to_owned(),
            calendars: BTreeMap::new(),
        }
    }
}

impl Settings {
    /// Applies `change` to the settings for calendar `id` under calendar account `account`, and
    /// under every other account that has them. The settings, and the account, are made if new.
    pub fn update_calendar(
        &mut self,
        account: &str,
        id: &str,
        change: impl Fn(&mut CalendarSetting),
    ) {
        let own = self
            .calendar_accounts
            .entry(account.to_owned())
            .or_insert_with(|| CalendarAccount {
                email: account.to_owned(),
                ..CalendarAccount::default()
            });
        change(
            own.calendars
                .entry(id.to_owned())
                .or_insert_with(|| CalendarSetting {
                    id: id.to_owned(),
                    ..CalendarSetting::default()
                }),
        );
        for (email, other) in &mut self.calendar_accounts {
            if email != account
                && let Some(setting) = other.calendars.get_mut(id)
            {
                change(setting);
            }
        }
    }

    /// The calendars of the signed-in calendar accounts whose settings pass `wanted`, each once.
    #[must_use]
    pub fn calendars_where(&self, wanted: impl Fn(&CalendarSetting) -> bool) -> Vec<String> {
        let ids: std::collections::BTreeSet<&String> = self
            .calendar_accounts
            .values()
            .filter(|account| account.authed)
            .flat_map(|account| account.calendars.iter())
            .filter(|(_, setting)| wanted(setting))
            .map(|(id, _)| id)
            .collect();
        ids.into_iter().cloned().collect()
    }
}

/// `calendarAccounts/{email}/calendars/{id}`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CalendarSetting {
    /// `id`.
    pub id: String,
    /// `sync`: its events are fetched.
    pub sync: bool,
    /// `show`: its events are on the grid.
    pub show: bool,
    /// `color`: one of the 25 calendar colour names.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// `checkForSchedulingConflicts`.
    pub check_for_scheduling_conflicts: bool,
    /// `notifications`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notifications: Option<CalendarNotifications>,
}

impl CalendarSetting {
    /// Its events notify on this device (`notifications/enabledDesktop`).
    pub fn notifies(&self) -> bool {
        self.notifications
            .as_ref()
            .is_some_and(|notifications| notifications.enabled_desktop)
    }
}

/// `calendarAccounts/{email}/calendars/{id}/notifications`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CalendarNotifications {
    /// `enabled`.
    pub enabled: bool,
    /// `enabledDesktop`.
    pub enabled_desktop: bool,
}

/// A nudge's entry in `nudges`, with its times as ISO timestamps.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Nudge {
    /// `firstNudgedAt`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_nudged_at: Option<String>,
    /// `lastCompletedAt`: when it was dismissed or taken up. After that it doesn't show again.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_completed_at: Option<String>,
}

impl Settings {
    /// Records that nudge `id` was dismissed or taken up `at`. If no time was recorded for when
    /// it was first shown, `at` is recorded as that too.
    pub fn complete_nudge(&mut self, id: &str, at: &str) {
        let nudge = self.nudges.entry(id.to_owned()).or_default();
        nudge.first_nudged_at.get_or_insert_with(|| at.to_owned());
        nudge.last_completed_at = Some(at.to_owned());
    }

    /// Whether nudge `id` was dismissed or taken up.
    pub fn nudge_completed(&self, id: &str) -> bool {
        self.nudges
            .get(id)
            .is_some_and(|nudge| nudge.last_completed_at.is_some())
    }
}

/// A recent search (`SearchHistoryItem`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchHistoryItem {
    /// The query, trimmed.
    pub value: String,
    /// What it was.
    #[serde(rename = "type")]
    pub kind: SearchHistoryKind,
    /// When it was last searched, as an ISO timestamp.
    pub timestamp: String,
}

/// What a recent search was: someone's address, or anything else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchHistoryKind {
    /// An address (a contact picked from the suggestions).
    Contact,
    /// Any other query.
    Query,
}

/// How many recent searches are kept.
const SEARCH_HISTORY: usize = 15;

/// `splitsEngagement`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SplitsEngagement {
    /// When a split (or Important and Other) was first turned on outside account setup, as an
    /// ISO timestamp. From then on the inbox header has no Add Split.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_split_enabled_at: Option<String>,
}

impl Settings {
    /// The splits, Important, Other and Inbox in their order, whether on or off. Important and
    /// Other are included only while they're on, and Inbox only while they're off.
    pub fn split_entries(&self) -> Vec<SplitEntry<'_>> {
        let shown = |entry: &SplitEntry<'_>| match entry {
            SplitEntry::System(SystemSplit::Inbox) => self.unified_inbox,
            SplitEntry::System(_) => !self.unified_inbox,
            SplitEntry::Split(_) => true,
        };
        if self.split_inboxes_v2.is_unset() {
            // Not set up yet ([`Settings::set_up_splits`]), so there are no splits.
            let systems: &[SystemSplit] = if self.unified_inbox {
                &[SystemSplit::Inbox]
            } else {
                &[SystemSplit::Important, SystemSplit::Other]
            };
            return systems.iter().copied().map(SplitEntry::System).collect();
        }
        self.split_inboxes_v2.entries().filter(shown).collect()
    }

    /// The splits, in order, on or off.
    pub fn splits(&self) -> impl Iterator<Item = &SplitInbox> {
        self.split_entries()
            .into_iter()
            .filter_map(|entry| match entry {
                SplitEntry::Split(split) => Some(split),
                SplitEntry::System(_) => None,
            })
    }

    /// The splits shown as tabs.
    pub fn enabled_splits(&self) -> impl Iterator<Item = &SplitInbox> {
        self.splits().filter(|split| !split.is_disabled)
    }

    /// The split whose list id is `id`.
    pub fn split(&self, id: &str) -> Option<&SplitInbox> {
        self.splits().find(|split| split.id == id)
    }

    /// Other's tab hides while it's empty (`splitInboxesV2.hideOtherWhenEmpty`).
    pub fn hides_other_when_empty(&self) -> bool {
        self.split_inboxes_v2.hide_other_when_empty.unwrap_or(false)
    }

    /// Sets whether Other's tab hides while it's empty.
    pub fn set_hide_other_when_empty(&mut self, hide: bool) {
        self.migrate_splits();
        self.split_inboxes_v2.hide_other_when_empty = Some(hide);
    }

    /// Gives `splitInboxesV2` its first order, with no splits, unless it already has an order.
    /// Returns whether it had none.
    fn migrate_splits(&mut self) -> bool {
        if !self.split_inboxes_v2.is_unset() {
            return false;
        }
        self.split_inboxes_v2 = SplitInboxesV2::with_splits(&[], self.unified_inbox);
        true
    }

    /// Sets up the account's splits the first time Exo sees them: adds the default splits
    /// (`defaults`, off). Does nothing after that first time, so a default split the user deletes
    /// stays deleted. Returns whether anything changed.
    pub fn set_up_splits(&mut self, defaults: impl FnOnce() -> Vec<SplitInbox>) -> bool {
        if !self.migrate_splits() {
            return false;
        }
        for split in defaults() {
            self.split_inboxes_v2.add(split, self.unified_inbox);
        }
        true
    }

    /// Adds `split`, last if it's off, or else where new tabs go.
    pub fn add_split(&mut self, split: SplitInbox) {
        self.migrate_splits();
        self.split_inboxes_v2.add(split, self.unified_inbox);
    }

    /// Puts `new` in place of the split whose list id is `id`. If `new` turns the split on, it
    /// moves to where new tabs go.
    pub fn replace_split(&mut self, id: &str, new: SplitInbox) {
        self.migrate_splits();
        self.split_inboxes_v2.replace(id, new, self.unified_inbox);
    }

    /// Changes the split whose list id is `id`, leaving it where it is.
    pub fn update_split(&mut self, id: &str, change: impl FnOnce(&mut SplitInbox)) {
        self.migrate_splits();
        self.split_inboxes_v2.update(id, change);
    }

    /// Deletes the split whose list id is `id` and returns it.
    pub fn remove_split(&mut self, id: &str) -> Option<SplitInbox> {
        self.migrate_splits();
        self.split_inboxes_v2.remove(id)
    }

    /// Moves the entry whose order id is `moved` to `pushed`'s place.
    pub fn reorder_splits(&mut self, moved: &str, pushed: &str) {
        self.migrate_splits();
        self.split_inboxes_v2
            .reorder(moved, pushed, self.unified_inbox);
    }

    /// Turns Important and Other on or off, moving them around the shown splits.
    pub fn toggle_important_other(&mut self) {
        self.migrate_splits();
        self.unified_inbox = !self.unified_inbox;
        self.split_inboxes_v2
            .toggle_important_other(self.unified_inbox);
    }

    /// Remembers a search. Searching for the same thing again moves it to the newest. Only the
    /// newest 15 are kept.
    pub fn remember_search(&mut self, value: &str, kind: SearchHistoryKind, now: jiff::Timestamp) {
        let value = value.trim();
        if value.is_empty() {
            return;
        }
        self.search_history
            .retain(|item| !(item.value == value && item.kind == kind));
        self.search_history.push(SearchHistoryItem {
            value: value.to_owned(),
            kind,
            timestamp: now.to_string(),
        });
        self.search_history
            .sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
        let excess = self.search_history.len().saturating_sub(SEARCH_HISTORY);
        self.search_history.drain(..excess);
    }

    /// Whether email notifications are on, which they are unless this device or the account
    /// turned them off (the Notifications dialog's checkbox).
    pub fn email_notifications(&self, device: &DeviceSettings) -> bool {
        !(device.disable_notifications || self.disable_notifications)
    }

    /// Whether replies and forwards carry the signature (`includeSignatureOnReplies`), which is
    /// on unless turned off.
    pub fn include_signature_on_replies(&self) -> bool {
        self.include_signature_on_replies.unwrap_or(true)
    }

    /// Whether a backtick is Escape in compose too (`enableBacktickAsEscape`), which is off
    /// unless turned on.
    pub fn backtick_as_escape(&self) -> bool {
        self.enable_backtick_as_escape.unwrap_or(false)
    }

    /// Whether autocorrect is on, as turned on or off in Exo, or else `system` (macOS's own
    /// setting).
    pub fn autocorrect(&self, system: bool) -> bool {
        self.typing_assistance.enabled.unwrap_or(system)
    }

    /// Whether autocomplete is on, which it is unless turned off.
    pub fn autocomplete(&self) -> bool {
        self.typing_assistance.autocomplete_enabled.unwrap_or(true)
    }

    /// Records that a split was turned on, the first time.
    pub fn engage_splits(&mut self) {
        if self.splits_engagement.first_split_enabled_at.is_none() {
            self.splits_engagement.first_split_enabled_at =
                Some(jiff::Timestamp::now().to_string());
        }
    }
}

impl Settings {
    /// Instant Intro's default line.
    pub const INSTANT_INTRO: &'static str = "Thanks {first_name}! (to BCC)";

    /// Instant Intro's line: `customInstantIntro`, or the default while it's empty.
    pub fn instant_intro(&self) -> &str {
        if self.custom_instant_intro.is_empty() {
            Self::INSTANT_INTRO
        } else {
            &self.custom_instant_intro
        }
    }
}

impl Document for Settings {
    const KEY: &'static str = "account";
}

/// `theme`, in the Classic style, the only style Exo has.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Theme {
    /// `snow`.
    Snow,
    /// `carbon`.
    Carbon,
    /// `auto`: Match System, the default for a new account or a document without a theme.
    #[default]
    Auto,
}

/// This device's settings for the account.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DeviceSettings {
    /// The newest "new setting" announcement the user has seen, by opening Settings.
    pub new_setting_notification: Option<String>,
    /// `disableNotifications`: email notifications off on this device.
    #[serde(skip_serializing_if = "is_false")]
    pub disable_notifications: bool,
    /// `localSendsCount`: messages sent from this device, which the follow-up hint waits on.
    #[serde(skip_serializing_if = "is_zero")]
    pub local_sends_count: u32,
    /// `firstCheckedEscape`: when Escape was first pressed with `enableBacktickAsEscape` unset on
    /// a Mac whose Escape is on its Touch Bar, in milliseconds since the epoch. The Touch Bar's
    /// tip waits 40 seconds from then.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_checked_escape: Option<i64>,
    /// `achievements`: the thresholds celebrated and the cards marked `New`.
    #[serde(skip_serializing_if = "Achievements::is_empty")]
    pub achievements: Achievements,
    /// The welcome splash is still to show, because the account signed in on this Mac for the
    /// first time and hasn't been past the splash.
    #[serde(skip_serializing_if = "is_false")]
    pub splash_pending: bool,
    /// The first-run flow reached account setup and hasn't been through it, so account setup
    /// opens again at the next launch.
    #[serde(skip_serializing_if = "is_false")]
    pub account_setup_pending: bool,
    /// Get Me To Zero has been run.
    #[serde(skip_serializing_if = "is_false")]
    pub gmtz_started: bool,
    /// The calendar accounts whose calendars the week's list folds away.
    #[serde(skip_serializing_if = "std::collections::BTreeSet::is_empty")]
    pub folded_calendar_accounts: std::collections::BTreeSet<String>,
}

impl Document for DeviceSettings {
    const KEY: &'static str = "device";
}

impl Store {
    /// The settings document `T`, with defaults for whatever was never set. A document that
    /// can't be read (written by another version of Exo) reads as the defaults.
    ///
    /// # Errors
    /// On database failures.
    pub async fn settings<T: Document>(&self) -> Result<T> {
        let mut db = self.inner.db.clone();
        read(&mut db).await
    }

    /// Saves the settings document `T`. Saving splits that route differently (one added,
    /// removed, turned on or off, or redefined) routes the inbox again.
    ///
    /// # Errors
    /// On database failures.
    pub async fn save_settings<T: Document>(&self, settings: &T) -> Result<()> {
        let value = serde_json::to_string(settings).expect("settings serialize");
        let mut db = self.inner.db.clone();
        let mut write = self.inner.begin(&mut db).await?;
        SettingsRow::upsert_by_key(T::KEY)
            .value(value)
            .exec(write.exec())
            .await?;
        write.commit().await?;
        if T::KEY == Settings::KEY {
            self.inner.reload_splits(true).await?;
        }
        Ok(())
    }
}

/// Reads the settings document `T` (see [`Store::settings`]).
pub(crate) async fn read<T: Document>(exec: &mut dyn Executor) -> Result<T> {
    let row = SettingsRow::filter_by_key(T::KEY)
        .first()
        .exec(exec)
        .await?;
    Ok(row
        .and_then(|row| {
            serde_json::from_str(&row.value)
                .inspect_err(|error| log::warn!("ignoring unreadable {} settings: {error}", T::KEY))
                .ok()
        })
        .unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn documents_use_their_key_names() {
        let settings = Settings {
            theme: Theme::Carbon,
            ..Settings::default()
        };
        let json = serde_json::to_value(settings).expect("serializes");
        assert_eq!(
            json,
            serde_json::json!({ "theme": "carbon", "unifiedInbox": false, "splitsEngagement": {}, "searchHistory": [] })
        );
        let device = DeviceSettings {
            new_setting_notification: Some("2023-06-13-salespackUI".into()),
            disable_notifications: true,
            local_sends_count: 6,
            first_checked_escape: Some(1_795_000_000_000),
            splash_pending: true,
            ..DeviceSettings::default()
        };
        let json = serde_json::to_value(device).expect("serializes");
        assert_eq!(
            json,
            serde_json::json!({ "newSettingNotification": "2023-06-13-salespackUI", "disableNotifications": true, "localSendsCount": 6, "firstCheckedEscape": 1_795_000_000_000_i64, "splashPending": true })
        );
    }

    #[test]
    fn compose_settings_use_their_key_names() {
        let settings: Settings = serde_json::from_value(serde_json::json!({
            "customInstantIntro": "Merci {first_name}",
            "emojiSkinTone": "MEDIUM_DARK",
            "autoBcc": ["123@bcc.hubspot.com"],
            "autoBccExclusions": ["acme.com"],
        }))
        .expect("parses");
        assert_eq!(settings.instant_intro(), "Merci {first_name}");
        assert_eq!(settings.emoji_skin_tone, Some(EmojiSkinTone::MediumDark));
        assert_eq!(
            (
                settings.auto_bcc.as_slice(),
                settings.auto_bcc_exclusions.as_slice()
            ),
            (
                ["123@bcc.hubspot.com".to_owned()].as_slice(),
                ["acme.com".to_owned()].as_slice()
            )
        );
        let fresh = Settings::default();
        assert_eq!(
            (fresh.instant_intro(), fresh.emoji_skin_tone),
            ("Thanks {first_name}! (to BCC)", None),
            "unset: the default line, no tone yet"
        );
        let json = serde_json::to_value(&settings).expect("serializes");
        assert_eq!(json["emojiSkinTone"], "MEDIUM_DARK");
    }

    #[test]
    fn notification_settings_use_their_key_names_and_defaults() {
        let settings: Settings = serde_json::from_value(serde_json::json!({ "disableNotifications": true, "notifications": { "calendarEvents": { "leadTimeInSeconds": 300 } } })).expect("parses");
        assert_eq!(settings.notifications.calendar_events.lead_time(), 300);
        let device = DeviceSettings::default();
        assert!(
            !settings.email_notifications(&device),
            "off for the account is off here too"
        );
        let on = Settings::default();
        assert!(on.email_notifications(&device));
        assert!(!on.email_notifications(&DeviceSettings {
            disable_notifications: true,
            ..DeviceSettings::default()
        }));
        assert_eq!(
            on.notifications.calendar_events.lead_time(),
            120,
            "two minutes unless set"
        );
        let json = serde_json::to_value(&settings).expect("serializes");
        assert_eq!(
            (
                &json["disableNotifications"],
                &json["notifications"]["calendarEvents"]["leadTimeInSeconds"]
            ),
            (&serde_json::json!(true), &serde_json::json!(300))
        );
    }

    #[test]
    fn triage_settings_use_their_key_names_and_defaults() {
        let unset = Settings::default();
        assert_eq!(
            (
                unset.desktop_auto_advance,
                unset.send_and_done,
                unset.rsvp_and_done,
                unset.show_sender_full_names
            ),
            (AutoAdvance::Down, false, false, false)
        );
        let json = serde_json::json!({ "desktopAutoAdvance": "off", "sendAndDone": true, "rsvpAndDone": true, "showSenderFullNames": true });
        let settings: Settings = serde_json::from_value(json.clone()).expect("parses");
        assert_eq!(settings.desktop_auto_advance.direction(), 0);
        let written = serde_json::to_value(&settings).expect("serializes");
        for key in [
            "desktopAutoAdvance",
            "sendAndDone",
            "rsvpAndDone",
            "showSenderFullNames",
        ] {
            assert_eq!(written[key], json[key], "{key}");
        }
        let up = Settings {
            desktop_auto_advance: AutoAdvance::Up,
            ..Settings::default()
        };
        assert_eq!(
            serde_json::to_value(&up).expect("serializes")["desktopAutoAdvance"],
            "up"
        );
        assert!(
            serde_json::to_value(&unset)
                .expect("serializes")
                .get("desktopAutoAdvance")
                .is_none(),
            "the default isn't written"
        );
    }

    #[test]
    fn snippets_keep_their_fields_and_the_draft_files_they_hold() {
        let snippet = Snippet {
            id: "draft00aaaaaaaaaaaaaa".into(),
            name: "intro".into(),
            body: "<div>Hi {first_name}</div>".into(),
            cc: vec![Address {
                name: None,
                email: "dan@brightmoor.co".into(),
            }],
            analytics: SnippetAnalytics {
                sends: 2,
                replies: 1,
                last_sent: "2026-11-17T13:00:00Z".into(),
                unreplied: vec![SnippetSend {
                    thread: "t1".into(),
                    message: "m1".into(),
                }],
            },
            ..Snippet::default()
        };
        let settings = Settings {
            snippets: vec![snippet.clone()],
            ..Settings::default()
        };
        let json = serde_json::to_value(&settings).expect("serializes");
        assert_eq!(json["snippets"][0]["name"], "intro");
        assert_eq!(
            json["snippets"][0]["sends"], 2,
            "the analytics sit beside the snippet's fields"
        );
        assert_eq!(json["snippets"][0]["unreplied"][0]["thread"], "t1");
        assert_eq!(
            json["snippets"][0]["from"],
            serde_json::Value::Null,
            "the draft's own From"
        );
        let back: Settings = serde_json::from_value(json).expect("reads back");
        assert_eq!(back.snippets, [snippet]);
    }

    #[test]
    fn typing_assistance_follows_the_system_until_set() {
        let mut settings = Settings::default();
        assert!(
            settings.autocorrect(true) && !settings.autocorrect(false) && settings.autocomplete()
        );
        assert!(
            !serde_json::to_value(&settings)
                .expect("serializes")
                .as_object()
                .expect("an object")
                .contains_key("typingAssistance")
        );
        settings.typing_assistance.enabled = Some(false);
        settings.typing_assistance.autocomplete_enabled = Some(false);
        let json = serde_json::to_value(&settings).expect("serializes");
        assert_eq!(
            json["typingAssistance"],
            serde_json::json!({ "enabled": false, "autocompleteEnabled": false })
        );
        let back: Settings = serde_json::from_value(json).expect("reads back");
        assert!(!back.autocorrect(true) && !back.autocomplete());
    }

    #[test]
    fn search_history_keeps_the_newest_fifteen_once_each() {
        let mut settings = Settings::default();
        let at = |second: i64| jiff::Timestamp::from_second(1_800_000_000 + second).expect("valid");
        for i in 0..20 {
            settings.remember_search(&format!("query {i}"), SearchHistoryKind::Query, at(i));
        }
        settings.remember_search(" query 7 ", SearchHistoryKind::Query, at(30));
        settings.remember_search(
            "ana@letters.anaruiz.com",
            SearchHistoryKind::Contact,
            at(31),
        );
        let values: Vec<&str> = settings
            .search_history
            .iter()
            .map(|item| item.value.as_str())
            .collect();
        assert_eq!(values.len(), 15);
        assert_eq!(
            values[13..],
            ["query 7", "ana@letters.anaruiz.com"],
            "newest last, a repeat moved"
        );
        assert_eq!(
            values.iter().filter(|value| **value == "query 7").count(),
            1
        );
        let json = serde_json::to_value(&settings.search_history[14]).expect("serializes");
        assert_eq!(
            json,
            serde_json::json!({ "value": "ana@letters.anaruiz.com", "type": "contact", "timestamp": "2027-01-15T08:00:31Z" })
        );
    }

    #[test]
    fn missing_keys_take_the_defaults() {
        let settings: Settings = serde_json::from_str("{}").expect("parses");
        assert_eq!(settings.theme, Theme::Auto, "Match System");
        assert!(!settings.unified_inbox, "Important and Other are on");
        assert_eq!(
            settings
                .split_entries()
                .iter()
                .map(SplitEntry::name)
                .collect::<Vec<_>>(),
            ["Important", "Other"]
        );
    }

    #[test]
    fn writing_toggles_use_their_key_names_and_defaults() {
        let settings: Settings = serde_json::from_str("{}").expect("parses");
        assert!(
            settings.include_signature_on_replies(),
            "on until turned off"
        );
        assert!(
            !settings.convert_hyphen_to_bullet_list && settings.enable_backtick_as_escape.is_none()
        );
        let json = r#"{"includeSignatureOnReplies":false,"convertHyphenToBulletList":true,"enableBacktickAsEscape":true}"#;
        let settings: Settings = serde_json::from_str(json).expect("parses");
        assert!(
            !settings.include_signature_on_replies()
                && settings.convert_hyphen_to_bullet_list
                && settings.backtick_as_escape()
        );
        let written = serde_json::to_value(&settings).expect("serializes");
        assert_eq!(written["includeSignatureOnReplies"], false);
        assert_eq!(written["convertHyphenToBulletList"], true);
        assert_eq!(written["enableBacktickAsEscape"], true);
    }

    fn defaults() -> Vec<SplitInbox> {
        let me = ["maya@brightmoor.co".to_owned()];
        SplitInbox::defaults(
            Some("brightmoor.co"),
            crate::split::Definitions {
                labels: &[],
                me: &me,
            },
        )
    }

    fn names(settings: &Settings) -> Vec<(&str, bool)> {
        settings
            .split_entries()
            .iter()
            .map(|entry| (entry.name(), entry.is_disabled()))
            .collect()
    }

    #[test]
    fn a_new_account_gets_the_default_splits_once() {
        let mut settings = Settings::default();
        assert!(settings.set_up_splits(defaults));
        let off = |name| (name, true);
        assert_eq!(
            names(&settings),
            [
                ("Important", false),
                ("Other", false),
                off("Starred"),
                off("VIP"),
                off("Team"),
                off("Calendar"),
                off("News")
            ]
        );
        let team = settings
            .splits()
            .find(|split| split.name() == "Team")
            .expect("Team");
        assert_eq!(
            (team.query(), team.leave_threads_in_important_other),
            ("from:brightmoor.co to:me", true)
        );
        assert_eq!(
            team.matcher.gmail_query,
            "-in:DRAFT -in:CHAT in:inbox from:\"brightmoor.co\" to:{maya@brightmoor.co}"
        );

        let news = settings
            .splits()
            .find(|split| split.name() == "News")
            .expect("News")
            .id
            .clone();
        settings.remove_split(&news);
        assert!(!settings.set_up_splits(defaults), "only the first time");
        assert!(
            settings.splits().all(|split| split.name() != "News"),
            "a deleted default stays deleted"
        );

        let json = serde_json::to_value(&settings).expect("serializes");
        assert_eq!(
            json["splitInboxesV2"]["splitInboxesOrder"]
                .as_array()
                .map(Vec::len),
            Some(7)
        );
        let back: Settings = serde_json::from_value(json).expect("parses");
        assert_eq!(back, settings);
    }
}
