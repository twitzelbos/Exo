//! Settings survive closing and reopening the store, each document on its own.

mod support;

use exo_store::{DeviceSettings, RemoteContentBlocking, Settings, Store, Theme};
use support::{ACCOUNT, TempDir};

async fn open(dir: &TempDir) -> Store {
    Store::open(
        dir.path(),
        ACCOUNT,
        gmail::Endpoints::google(),
        gmail::StaticToken("unused".into()),
    )
    .await
    .expect("store opens")
}

#[tokio::test]
async fn settings_round_trip_through_a_reopen() {
    let dir = TempDir::new();
    let store = open(&dir).await;
    assert_eq!(
        store.settings::<Settings>().await.expect("reads"),
        Settings::default(),
        "a new account has the defaults"
    );
    assert_eq!(Settings::default().theme, Theme::Auto, "Match System");

    let mut settings = Settings::default();
    settings.theme = Theme::Carbon;
    store.save_settings(&settings).await.expect("saves");
    let device = DeviceSettings {
        new_setting_notification: Some("2023-06-13-salespackUI".into()),
        disable_notifications: true,
        local_sends_count: 6,
        first_checked_escape: None,
        splash_pending: true,
        ..DeviceSettings::default()
    };
    store.save_settings(&device).await.expect("saves");
    drop(store);

    let store = open(&dir).await;
    assert_eq!(store.settings::<Settings>().await.expect("reads"), settings);
    assert_eq!(
        store.settings::<DeviceSettings>().await.expect("reads"),
        device
    );
}

#[tokio::test]
async fn the_images_setting_is_kept_under_its_key_and_values() {
    let dir = TempDir::new();
    let fresh = open(&dir)
        .await
        .settings::<Settings>()
        .await
        .expect("reads");
    assert_eq!(fresh.remote_content_blocking, None, "never chosen");
    assert_eq!(
        fresh.remote_content_blocking.unwrap_or_default(),
        RemoteContentBlocking::All,
        "Exo's default: block all remote images until shown"
    );
    for (choice, value) in [
        (RemoteContentBlocking::All, "all"),
        (RemoteContentBlocking::None, "none"),
        (RemoteContentBlocking::ReduceTracking, "reduce-tracking"),
    ] {
        let mut settings = Settings::default();
        settings.remote_content_blocking = Some(choice);
        assert_eq!(
            serde_json::to_value(&settings).expect("serializes")["remoteContentBlocking"],
            value
        );
        open(&dir)
            .await
            .save_settings(&settings)
            .await
            .expect("saves");
        assert_eq!(
            open(&dir)
                .await
                .settings::<Settings>()
                .await
                .expect("reads")
                .remote_content_blocking,
            Some(choice),
            "{value} survives a reopen"
        );
    }
}

#[tokio::test]
async fn deleting_an_account_leaves_the_others_alone() {
    let dir = TempDir::new();
    let open_as = |account: &'static str| {
        Store::open(
            dir.path(),
            account,
            gmail::Endpoints::google(),
            gmail::StaticToken("unused".into()),
        )
    };
    let mut settings = Settings::default();
    settings.theme = Theme::Carbon;
    let (maya, bea) = (
        open_as(ACCOUNT).await.expect("opens"),
        open_as("bea@example.com").await.expect("opens"),
    );
    for store in [&maya, &bea] {
        store.save_settings(&settings).await.expect("saves");
        std::fs::create_dir_all(store.draft_files()).expect("a files folder");
        std::fs::write(store.draft_files().join("uuid"), "bytes").expect("an attachment");
    }
    let maya_files = maya.draft_files().to_owned();
    drop((maya, bea));

    Store::delete(dir.path(), ACCOUNT).expect("deletes");
    let left: Vec<String> = std::fs::read_dir(dir.path())
        .expect("lists")
        .map(|entry| {
            entry
                .expect("an entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert!(
        left.iter().all(|name| name.starts_with("bea@example.com.")),
        "only Bea's files are left: {left:?}"
    );
    assert!(!maya_files.exists());
    assert_eq!(
        open(&dir)
            .await
            .settings::<Settings>()
            .await
            .expect("reads"),
        Settings::default(),
        "Maya starts over"
    );
    assert_eq!(
        open_as("bea@example.com")
            .await
            .expect("opens")
            .settings::<Settings>()
            .await
            .expect("reads"),
        settings
    );

    Store::delete(dir.path(), "nobody@example.com").expect("nothing to delete is fine");
    for name in ["", "../bea@example.com", ".hidden", "a/b"] {
        assert!(
            matches!(
                Store::delete(dir.path(), name),
                Err(exo_store::Error::InvalidAccount(_))
            ),
            "{name:?}"
        );
    }
}
