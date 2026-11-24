//! Brand logos from BIMI, which show where a contact's photo would for a sender Google has no
//! photo of: the contact pane, notifications and the calendar's guests.
//!
//! A domain earns its logo through its mail. A message Gmail recorded as passing DMARC under a
//! policy that quarantines or rejects all of the mail that fails ([`records::passed`], the
//! domain's `_dmarc` record) sends Exo to the domain's BIMI record (`default._bimi`, through
//! macOS's resolver). Its logo shows only with a Verified Mark or Common Mark Certificate that
//! vouches for it ([`certificate`]), so another domain can't show a brand it doesn't own. Lookups
//! are kept with the account (`exo_store::BrandRecord`) and made again after the record's TTL, at
//! most once a day. The logo and certificate are fetched once (HTTPS only), and the logo, drawn,
//! is kept with the photos until the certificate expires, or until its CA's revocation list,
//! fetched about once a day, revokes it. Nothing a message names is ever fetched: the record comes
//! from DNS. `Block all remote images` stops these DNS lookups and fetches, as the sender's DNS
//! names their URLs, and leaves the logos already validated.

mod certificate;
mod dns;
mod records;

use std::{io::Read as _, path::PathBuf, time::Duration};

use exo_store::{BrandCheck, BrandRecord, Header, RemoteContentBlocking, Settings, Store};
use exo_ui::AvatarPhoto;
use jiff::Timestamp;

use crate::avatar_cache::{self, AvatarCache};

/// The shortest a lookup is kept: a record is looked up again at most once a day. A CA's
/// revocation list is kept as long, and the logos are checked against it as often.
const KEPT: Duration = Duration::from_secs(24 * 60 * 60);
/// How long fetching a brand's logo or certificate, or a revocation list, may take.
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);
/// The most a logo, certificate or revocation list may weigh: eight times the 32 KB Gmail
/// recommends for a logo.
const MAX_FILE: usize = 256 << 10;

const CACHE_ID: &str = "exo::brands";

/// `(account, domain)`, both lower-cased, scoping validated brand logos per signed-in account.
type BrandKey = (String, String);

#[derive(Clone, Default)]
struct ActiveAccount(Option<String>);

fn cache(ctx: &egui::Context) -> AvatarCache<BrandKey> {
    AvatarCache::of(ctx, CACHE_ID)
}

fn active_id() -> egui::Id {
    egui::Id::new("exo::brands::active")
}

/// Records which signed-in account is drawing on `ctx` this frame, for [`active_logo`].
pub(crate) fn set_active_account(ctx: &egui::Context, account: &str) {
    let lower = account.to_lowercase();
    ctx.data_mut(|data| data.insert_temp(active_id(), ActiveAccount(Some(lower))));
}

/// The validated logo of `email`'s domain in `account`, once mail from it in `account` has earned
/// one.
pub(crate) fn logo(ctx: &egui::Context, account: &str, email: &str) -> Option<AvatarPhoto> {
    let (_, domain) = email.rsplit_once('@')?;
    cache(ctx).photo(&(account.to_lowercase(), domain.to_lowercase()))
}

/// [`logo`] for whichever account [`set_active_account`] marked as on screen.
pub(crate) fn active_logo(ctx: &egui::Context, email: &str) -> Option<AvatarPhoto> {
    let active = ctx
        .data(|data| data.get_temp::<ActiveAccount>(active_id()))
        .and_then(|active| active.0)?;
    logo(ctx, &active, email)
}

/// `account` closed or cleared its cache: its validated brand logos show no more.
pub(crate) fn forget(ctx: &egui::Context, account: &str) {
    let lower = account.to_lowercase();
    cache(ctx).retain(|(owner, _)| owner != &lower);
    ctx.data_mut(|data| {
        if data
            .get_temp::<ActiveAccount>(active_id())
            .and_then(|active| active.0)
            .as_deref()
            == Some(lower.as_str())
        {
            data.insert_temp(active_id(), ActiveAccount(None));
        }
    });
}

/// Whether `headers` pass DMARC under an enforcing policy aligned with `email`'s domain.
pub(crate) fn passed_for(headers: &[Header], email: &str) -> bool {
    let Some((_, domain)) = email.rsplit_once('@') else {
        return false;
    };
    records::passed(headers).is_some_and(|passed| passed.domain.eq_ignore_ascii_case(domain.trim()))
}

/// Whether `detail`'s message on screen for `email` (`focused` first, else their newest received
/// message in the conversation) passed DMARC: `Some(false)` suppresses any cached logo for a
/// spoofed message, `Some(true)` allows it, and `None` means `detail` has no received message from
/// `email`.
pub(crate) fn allowed_in_thread(
    detail: &exo_store::ThreadDetail,
    focused: Option<&str>,
    email: &str,
) -> Option<bool> {
    let from_sender = |message: &&exo_store::MessageDetail| {
        !message.is_draft()
            && message
                .from
                .as_ref()
                .is_some_and(|from| from.email.eq_ignore_ascii_case(email))
    };
    let message = focused
        .and_then(|id| detail.messages.iter().find(|message| message.id == id))
        .filter(from_sender)
        .or_else(|| detail.messages.iter().rev().find(from_sender))?;
    Some(passed_for(&message.headers, email))
}

/// An account's lookups: its store, which keeps them, its photo cache, which keeps the logos, and
/// the context whose avatars show them.
#[derive(Clone)]
pub(crate) struct Brands {
    ctx: egui::Context,
    account: String,
    store: Store,
    dir: PathBuf,
    /// For logos and certificates, over HTTPS only.
    http: crate::remote_content::Client,
    /// For revocation lists, which CAs serve over plain HTTP: they're signed.
    lists: crate::remote_content::Client,
    /// Whether DNS may be asked. Not on mock-servers, whose domains aren't the world's.
    lookups: bool,
}

impl Brands {
    /// `account`'s brands in `store`, whose logos are kept in `dir`, the account's photo cache.
    pub(crate) fn new(
        ctx: &egui::Context,
        account: &str,
        store: Store,
        dir: PathBuf,
        lookups: bool,
    ) -> Self {
        Self {
            ctx: ctx.clone(),
            account: account.to_lowercase(),
            store,
            dir,
            http: crate::remote_content::Client::https_only(FETCH_TIMEOUT),
            lists: crate::remote_content::Client::new(FETCH_TIMEOUT, false),
            lookups,
        }
    }

    /// The context whose avatars show the logos.
    pub(crate) fn ctx(&self) -> &egui::Context {
        &self.ctx
    }

    /// The validated logo of `email`'s domain in this account, if already loaded.
    pub(crate) fn logo(&self, email: &str) -> Option<AvatarPhoto> {
        logo(&self.ctx, &self.account, email)
    }

    /// [`Self::logo_for`] when `headers` pass DMARC aligned with `email`'s domain.
    pub(crate) async fn logo_for_sender(&self, email: &str, headers: &[Header]) -> Option<Vec<u8>> {
        if !passed_for(headers, email) {
            return None;
        }
        self.logo_for(headers).await
    }

    /// The logo (a PNG) of the brand that sent the message with `headers`, looked up first when
    /// that's due, which shows wherever its people are drawn ([`logo`]) from then on. `None` when
    /// the message or its domain doesn't earn one.
    pub(crate) async fn logo_for(&self, headers: &[Header]) -> Option<Vec<u8>> {
        self.logo_for_with(headers, look_up).await
    }

    async fn logo_for_with<F>(&self, headers: &[Header], lookup: F) -> Option<Vec<u8>>
    where
        F: AsyncFnOnce(&str, Timestamp) -> Option<BrandRecord>,
    {
        let domain = records::passed(headers)?.domain;
        let now = Timestamp::now();
        let kept = match self.store.brands().await {
            Ok(mut brands) => brands.remove(&domain),
            Err(error) => {
                log::warn!("reading the brands looked up: {error}");
                None
            }
        };
        let mut record = match kept {
            Some(record) if record.next > now => record,
            kept => {
                if self.blocked().await {
                    kept?
                } else if !self.lookups {
                    return None;
                } else {
                    let record = carried(kept, lookup(&domain, now).await?, now);
                    self.save(&domain, &record).await;
                    record
                }
            }
        };
        let png = self.checked_logo(&domain, &mut record, now).await;
        let photo = png
            .as_deref()
            .and_then(|png| avatar_cache::decode(&self.ctx, "brand logo", png))
            .map(|photo| (photo, ()));
        // A domain whose record or certificate no longer earns it a logo is removed.
        cache(&self.ctx).set((self.account.clone(), domain), photo);
        png
    }

    /// Shows the logos earlier runs validated, then checks them against their CAs' revocation
    /// lists, at once and each day after.
    pub(crate) async fn watch(&self) {
        self.restore().await;
        loop {
            self.recheck().await;
            tokio::time::sleep(KEPT).await;
        }
    }

    /// Shows the logos kept from earlier runs whose certificates are still in date.
    async fn restore(&self) {
        let brands = match self.store.brands().await {
            Ok(brands) => brands,
            Err(error) => {
                log::warn!("reading the brands looked up: {error}");
                return;
            }
        };
        let now = Timestamp::now();
        let logos: Vec<_> = brands
            .into_iter()
            .filter(|(_, record)| is_valid(record.checked, now))
            .filter_map(|(domain, record)| {
                let (png, _) = self.kept(&record)?;
                Some((
                    (self.account.clone(), domain),
                    avatar_cache::decode(&self.ctx, "brand logo", &png)?,
                    (),
                ))
            })
            .collect();
        cache(&self.ctx).extend(logos);
    }

    /// Checks the certificates of the logos validated so far against their CAs' revocation lists.
    /// A revoked certificate's logo goes, and its record waits for its next lookup, as a refused
    /// one does.
    async fn recheck(&self) {
        let brands = match self.store.brands().await {
            Ok(brands) => brands,
            Err(error) => {
                log::warn!("reading the brands looked up: {error}");
                return;
            }
        };
        let now = Timestamp::now();
        for (domain, mut record) in brands {
            if !is_valid(record.checked, now) {
                continue;
            }
            let Some((_, pem)) = self.kept(&record) else {
                continue;
            };
            if self.revoked(&pem, now).await != Some(true) {
                continue;
            }
            log::info!("{domain}'s certificate is revoked: its logo goes");
            record.checked = Some(BrandCheck::Invalid);
            self.save(&domain, &record).await;
            cache(&self.ctx).remove(&(self.account.clone(), domain));
        }
    }

    /// The logo `record` names, drawn, when its certificate vouches for it: the copy kept with the
    /// photos, else fetched and checked, which `record` keeps the outcome of. A fetch that fails,
    /// like a certificate that doesn't vouch for the logo, is tried again when the record is next
    /// looked up. The sender's servers answer these fetches, so one that always failed would
    /// otherwise hear from Exo with each of the domain's messages.
    async fn checked_logo(
        &self,
        domain: &str,
        record: &mut BrandRecord,
        now: Timestamp,
    ) -> Option<Vec<u8>> {
        let (Some(logo), Some(certificate)) = (record.logo.clone(), record.certificate.clone())
        else {
            return None;
        };
        let kept = self.kept(record);
        match step(record.checked, kept.is_some(), self.blocked().await, now) {
            Step::Kept => return kept.map(|(png, _)| png),
            Step::Nothing => return None,
            Step::Fetch => {}
        }
        let (svg, pem) = futures::join!(fetch(&self.http, &logo), fetch(&self.http, &certificate));
        let drawn = match svg.and_then(uncompressed).zip(pem) {
            Some((svg, pem)) => self.vouched(domain, &svg, &pem, now).await,
            None => None,
        };
        record.checked = Some(match &drawn {
            Some((until, png, pem)) => {
                for (url, bytes) in [(&logo, png), (&certificate, pem)] {
                    if let Err(error) = crate::people::save(&self.file(url), bytes) {
                        log::warn!("keeping {domain}'s logo: {error}");
                    }
                }
                BrandCheck::Valid { until: *until }
            }
            None => BrandCheck::Invalid,
        });
        self.save(domain, record).await;
        drawn.map(|(_, png, _)| png)
    }

    /// `svg` drawn, with until when the certificate `pem` vouches for it as `domain`'s logo, and
    /// `pem` itself, unless the certificate doesn't vouch for it or its CA has revoked it. A
    /// revocation list that can't be had doesn't stand in the way.
    async fn vouched(
        &self,
        domain: &str,
        svg: &[u8],
        pem: &[u8],
        now: Timestamp,
    ) -> Option<(Timestamp, Vec<u8>, Vec<u8>)> {
        let refused = |why: &dyn std::fmt::Display| {
            log::info!("not showing {domain}'s logo: {why}");
        };
        let until = certificate::validate(pem, svg, domain, now)
            .inspect_err(|invalid| refused(invalid))
            .ok()?;
        if self.revoked(pem, now).await == Some(true) {
            refused(&"its certificate is revoked");
            return None;
        }
        let png = draw(svg);
        if png.is_none() {
            refused(&"it doesn't draw");
        }
        Some((until, png?, pem.to_vec()))
    }

    /// Whether the CA of the certificate in `pem` has revoked it, by its revocation list
    /// ([`revocation_list`](Self::revocation_list)). `None` when the list can't be had or can't
    /// tell, which leaves the logo as the rest of its check left it, until a list that tells.
    async fn revoked(&self, pem: &[u8], now: Timestamp) -> Option<bool> {
        let list = self
            .revocation_list(&certificate::revocation_lists(pem))
            .await?;
        certificate::revoked(pem, &list, now)
    }

    /// The revocation list at `urls`, a CA's and its mirrors: the copy kept with the photos while
    /// it's under a day old, else the first of them that answers, kept, else the kept copy however
    /// old (one past its next update can't tell). Under `Block all remote images`, the kept copy.
    async fn revocation_list(&self, urls: &[String]) -> Option<Vec<u8>> {
        let file = self.file(urls.first()?);
        let kept = std::fs::read(&file).ok();
        let age = std::fs::metadata(&file)
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok());
        let fresh = kept.is_some() && age.is_some_and(|age| age < KEPT);
        if fresh || self.blocked().await {
            return kept;
        }
        for url in urls {
            if let Some(list) = fetch(&self.lists, url).await {
                if let Err(error) = crate::people::save(&file, &list) {
                    log::warn!("keeping a revocation list: {error}");
                }
                return Some(list);
            }
        }
        kept
    }

    /// Whether the account blocks all remote images, which stops these fetches too: the sender's
    /// DNS names their URLs. Read as each fetch is due, so a change applies at once. When the
    /// setting can't be read, nothing is fetched.
    async fn blocked(&self) -> bool {
        match self.store.settings::<Settings>().await {
            Ok(settings) => {
                settings.remote_content_blocking.unwrap_or_default() == RemoteContentBlocking::All
            }
            Err(error) => {
                log::warn!("reading the images setting: {error}");
                true
            }
        }
    }

    /// The drawn logo and the certificate `record` names, as kept with the photos.
    fn kept(&self, record: &BrandRecord) -> Option<(Vec<u8>, Vec<u8>)> {
        let png = std::fs::read(self.file(record.logo.as_deref()?)).ok()?;
        let pem = std::fs::read(self.file(record.certificate.as_deref()?)).ok()?;
        Some((png, pem))
    }

    /// Where what came from `url` is kept: a logo (drawn), a certificate or a revocation list.
    fn file(&self, url: &str) -> PathBuf {
        self.dir.join(crate::people::file_name(url))
    }

    async fn save(&self, domain: &str, record: &BrandRecord) {
        if let Err(error) = self.store.save_brand(domain, record).await {
            log::warn!("keeping a brand looked up: {error}");
        }
    }
}

/// The file at `url`, through `http`, up to [`MAX_FILE`].
async fn fetch(http: &crate::remote_content::Client, url: &str) -> Option<Vec<u8>> {
    match http.bytes(url, MAX_FILE).await {
        Ok(Some(bytes)) => Some(bytes),
        Ok(None) => {
            log::info!("not fetching {url}: over {MAX_FILE} bytes");
            None
        }
        Err(error) => {
            log::info!("fetching {url}: {error}");
            None
        }
    }
}

/// What showing a brand's logo takes next.
#[derive(Debug, PartialEq, Eq)]
enum Step {
    /// The copy kept with the photos shows.
    Kept,
    /// The logo and certificate are fetched and checked.
    Fetch,
    /// Nothing shows.
    Nothing,
}

/// The step for a brand whose last check came to `checked`, whose logo and certificate are
/// `kept` with the photos or not, while remote images are `blocked` or not.
fn step(checked: Option<BrandCheck>, kept: bool, blocked: bool, now: Timestamp) -> Step {
    match checked {
        // A refused certificate, or files that couldn't be had, wait for the next lookup.
        Some(BrandCheck::Invalid) => Step::Nothing,
        checked if kept && is_valid(checked, now) => Step::Kept,
        _ if blocked => Step::Nothing,
        _ => Step::Fetch,
    }
}

/// Whether `checked` says a certificate vouches for its logo at `now`.
fn is_valid(checked: Option<BrandCheck>, now: Timestamp) -> bool {
    matches!(checked, Some(BrandCheck::Valid { until }) if until > now)
}

/// `fresh`, a new lookup of the domain whose last one is `kept`, keeping what `kept`'s check came
/// to while the record names the same files and the certificate is in date, so they aren't
/// fetched again.
fn carried(kept: Option<BrandRecord>, mut fresh: BrandRecord, now: Timestamp) -> BrandRecord {
    if let Some(kept) = kept
        && (&kept.logo, &kept.certificate) == (&fresh.logo, &fresh.certificate)
        && is_valid(kept.checked, now)
    {
        fresh.checked = kept.checked;
    }
    fresh
}

/// `domain`'s brand as DNS has it now: its BIMI record, when its DMARC policy applies to all of its
/// failing mail, and when to look again. `None` when DNS doesn't answer, to ask again next time.
async fn look_up(domain: &str, now: Timestamp) -> Option<BrandRecord> {
    // A domain without records of its own takes its organizational domain's.
    let organization = psl::domain_str(domain).unwrap_or(domain).to_owned();
    let lookup = async |prefix: &str| -> Result<Option<dns::Txt>, ()> {
        match dns::txt(&format!("{prefix}.{domain}")).await? {
            None if organization != domain => dns::txt(&format!("{prefix}.{organization}")).await,
            found => Ok(found),
        }
    };
    let dmarc = lookup("_dmarc").await.ok()?;
    let applies = dmarc.as_ref().is_some_and(|dmarc| {
        dmarc
            .records
            .iter()
            .any(|record| records::dmarc_applies_to_all(record))
    });
    let bimi = if applies {
        lookup("default._bimi").await.ok()?
    } else {
        None
    };
    let ttl = bimi
        .as_ref()
        .or(dmarc.as_ref())
        .map_or(KEPT, |found| found.ttl.max(KEPT));
    let record = bimi.and_then(|bimi| bimi.records.iter().find_map(|record| records::bimi(record)));
    Some(BrandRecord {
        next: now + jiff::SignedDuration::try_from(ttl).unwrap_or_default(),
        logo: record.as_ref().and_then(|record| record.logo.clone()),
        certificate: record.and_then(|record| record.certificate),
        checked: None,
    })
}

/// `logo` uncompressed when it's SVGZ, up to [`MAX_FILE`].
fn uncompressed(logo: Vec<u8>) -> Option<Vec<u8>> {
    if !logo.starts_with(&[0x1f, 0x8b]) {
        return Some(logo);
    }
    let mut svg = Vec::new();
    flate2::read::GzDecoder::new(logo.as_slice())
        .take(MAX_FILE as u64 + 1)
        .read_to_end(&mut svg)
        .ok()?;
    (svg.len() <= MAX_FILE).then_some(svg)
}

/// `svg` drawn as a PNG the size of the photos, fitted and centred on white: Gmail asks for a
/// square logo on a solid background, and one that leaves its background clear shows on white
/// rather than on the default avatar. It draws nothing from elsewhere, images or files.
fn draw(svg: &[u8]) -> Option<Vec<u8>> {
    use resvg::{tiny_skia, usvg};

    let options = usvg::Options {
        resources_dir: None,
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_data: Box::new(|_, _, _| None),
            resolve_string: Box::new(|_, _| None),
        },
        ..usvg::Options::default()
    };
    let tree = usvg::Tree::from_data(svg, &options).ok()?;
    let mut pixmap = tiny_skia::Pixmap::new(crate::people::SIZE, crate::people::SIZE)?;
    pixmap.fill(tiny_skia::Color::WHITE);
    let (side, size) = (crate::people::SIZE as f32, tree.size());
    let scale = (side / size.width()).min(side / size.height());
    let transform = tiny_skia::Transform::from_translate(
        (side - size.width() * scale) / 2.0,
        (side - size.height() * scale) / 2.0,
    )
    .pre_scale(scale, scale);
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    pixmap.encode_png().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(time: &str) -> Timestamp {
        time.parse().expect("a time")
    }

    fn record(logo: &str, checked: Option<BrandCheck>) -> BrandRecord {
        BrandRecord {
            next: at("2026-06-02T00:00:00Z"),
            logo: Some(format!("https://brand.example/{logo}.svg")),
            certificate: Some("https://brand.example/vmc.pem".to_owned()),
            checked,
        }
    }

    #[test]
    fn a_valid_certificate_isnt_fetched_again_while_the_record_names_it() {
        let now = at("2026-06-01T00:00:00Z");
        let valid = Some(BrandCheck::Valid {
            until: at("2027-01-01T00:00:00Z"),
        });
        let kept = |checked| Some(record("logo", checked));
        assert_eq!(
            carried(kept(valid), record("logo", None), now).checked,
            valid
        );
        // A new logo, an expired certificate or one that didn't validate are checked again.
        assert_eq!(carried(kept(valid), record("new", None), now).checked, None);
        let later = at("2027-02-01T00:00:00Z");
        assert_eq!(
            carried(kept(valid), record("logo", None), later).checked,
            None
        );
        let invalid = Some(BrandCheck::Invalid);
        assert_eq!(
            carried(kept(invalid), record("logo", None), now).checked,
            None
        );
        assert_eq!(carried(None, record("logo", None), now).checked, None);
    }

    #[test]
    fn blocking_all_remote_images_stops_the_fetches_but_not_a_kept_logo() {
        let now = at("2026-06-01T00:00:00Z");
        let valid = Some(BrandCheck::Valid {
            until: at("2027-01-01T00:00:00Z"),
        });
        let expired = Some(BrandCheck::Valid {
            until: at("2026-05-01T00:00:00Z"),
        });
        for blocked in [false, true] {
            assert_eq!(step(valid, true, blocked, now), Step::Kept);
            assert_eq!(
                step(Some(BrandCheck::Invalid), true, blocked, now),
                Step::Nothing
            );
        }
        // What isn't kept, or no longer vouches, is fetched again, unless images are blocked.
        for (checked, kept) in [(None, false), (valid, false), (expired, true)] {
            assert_eq!(step(checked, kept, false, now), Step::Fetch);
            assert_eq!(step(checked, kept, true, now), Step::Nothing);
        }
    }

    #[test]
    fn a_logo_is_drawn_fitted_and_centred_on_white() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 50"><rect width="100" height="50" fill="#ff0000"/></svg>"##;
        let png = draw(svg).expect("a logo");
        let image = image::load_from_memory(&png).expect("a PNG").into_rgba8();
        let side = crate::people::SIZE;
        assert_eq!(image.dimensions(), (side, side));
        assert_eq!(image.get_pixel(side / 2, side / 2).0, [255, 0, 0, 255]);
        assert_eq!(image.get_pixel(side / 2, 4).0, [255, 255, 255, 255]);
        assert!(draw(b"<svg").is_none());
    }

    #[test]
    fn an_svgz_logo_is_uncompressed() {
        use std::io::Write as _;

        let svg = b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>".to_vec();
        let mut svgz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        svgz.write_all(&svg).expect("compressed");
        let svgz = svgz.finish().expect("compressed");
        assert_eq!(uncompressed(svgz), Some(svg.clone()));
        assert_eq!(uncompressed(svg.clone()), Some(svg));
    }

    #[test]
    fn validated_logos_stay_isolated_per_account_and_forget_cleanly() {
        let ctx = egui::Context::default();
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><rect width="10" height="10" fill="#00aa55"/></svg>"##;
        let png = draw(svg).expect("drawn");
        let photo = avatar_cache::decode(&ctx, "brand logo", &png).expect("decoded");

        cache(&ctx).set(
            ("alice@work.example".into(), "paypal.com".into()),
            Some((photo, ())),
        );

        assert!(logo(&ctx, "Alice@Work.example", "service@paypal.com").is_some());
        assert!(
            logo(&ctx, "bob@private.example", "service@paypal.com").is_none(),
            "another signed-in account never sees Alice's cached brand logo"
        );

        set_active_account(&ctx, "bob@private.example");
        assert!(active_logo(&ctx, "service@paypal.com").is_none());
        set_active_account(&ctx, "alice@work.example");
        assert!(active_logo(&ctx, "service@paypal.com").is_some());

        forget(&ctx, "alice@work.example");
        assert!(logo(&ctx, "alice@work.example", "service@paypal.com").is_none());
        assert!(active_logo(&ctx, "service@paypal.com").is_none());
    }

    #[test]
    fn spoofed_messages_suppress_cached_brand_logos_in_threads() {
        use exo_store::{Body, Contact, MessageDetail, ThreadDetail, ThreadId};

        let message = |id: &str, email: &str, headers: Vec<Header>| MessageDetail {
            id: id.into(),
            labels: vec!["INBOX".into()],
            date: Timestamp::UNIX_EPOCH,
            from: Some(Contact {
                name: Some("PayPal".into()),
                email: email.into(),
                is_me: false,
            }),
            to: Vec::new(),
            cc: Vec::new(),
            bcc: Vec::new(),
            reply_to: Vec::new(),
            subject: "Alert".into(),
            snippet: String::new(),
            rfc822_id: None,
            in_reply_to: None,
            references: Vec::new(),
            list_id: None,
            list_unsubscribe: None,
            list_unsubscribe_post: None,
            headers,
            body: Body::default(),
            attachments: Vec::new(),
            failure: None,
            scheduled: None,
            sending: false,
        };
        let hdr = |name: &str, value: &str| Header {
            name: name.into(),
            value: value.into(),
        };
        let legit = message(
            "m1",
            "service@paypal.com",
            vec![
                hdr("From", "PayPal <service@paypal.com>"),
                hdr(
                    "Authentication-Results",
                    "mx.google.com; dmarc=pass (p=REJECT sp=REJECT) header.from=paypal.com",
                ),
            ],
        );
        let failed = message(
            "m2",
            "security@paypal.com",
            vec![
                hdr("From", "PayPal Security <security@paypal.com>"),
                hdr(
                    "Authentication-Results",
                    "mx.google.com; dmarc=fail (p=REJECT sp=REJECT) header.from=paypal.com",
                ),
            ],
        );
        let misaligned = message(
            "m3",
            "security@paypal.com",
            vec![
                hdr("From", "PayPal Security <security@paypal.com>"),
                hdr(
                    "Authentication-Results",
                    "mx.google.com; dmarc=pass (p=REJECT sp=REJECT) header.from=evil.example",
                ),
            ],
        );
        let detail = ThreadDetail {
            id: ThreadId::new("t1"),
            subject: "Alert".into(),
            labels: vec!["INBOX".into()],
            muted: false,
            messages: vec![legit, failed, misaligned],
            reminder: None,
        };
        assert_eq!(
            allowed_in_thread(&detail, Some("m1"), "service@paypal.com"),
            Some(true)
        );
        assert_eq!(
            allowed_in_thread(&detail, Some("m2"), "security@paypal.com"),
            Some(false)
        );
        assert_eq!(
            allowed_in_thread(&detail, Some("m3"), "security@paypal.com"),
            Some(false)
        );
        assert_eq!(
            allowed_in_thread(&detail, None, "security@paypal.com"),
            Some(false)
        );
        assert_eq!(allowed_in_thread(&detail, None, "other@stripe.com"), None);
    }

    #[tokio::test]
    async fn blocking_all_remote_images_skips_bimi_dns_lookups() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };

        let root = std::env::temp_dir().join(format!(
            "exo-brands-blocked-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let photos = root.join("photos");
        std::fs::create_dir_all(&photos).expect("photos dir");
        let store = Store::open(
            &root,
            "maya@brightmoor.co",
            gmail::Endpoints::google(),
            gmail::StaticToken("unused".into()),
        )
        .await
        .expect("opens store");
        let mut settings = Settings::default();
        settings.remote_content_blocking = Some(RemoteContentBlocking::All);
        store
            .save_settings(&settings)
            .await
            .expect("saves settings");

        let brands = Brands::new(
            &egui::Context::default(),
            "maya@brightmoor.co",
            store.clone(),
            photos,
            true,
        );
        let headers = vec![
            Header {
                name: "From".into(),
                value: "Brand <hello@brand.example>".into(),
            },
            Header {
                name: "Authentication-Results".into(),
                value: "mx.google.com; dmarc=pass (p=REJECT sp=REJECT) header.from=brand.example"
                    .into(),
            },
        ];

        // With no cached record and RemoteContentBlocking::All, DNS lookup is skipped.
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = calls.clone();
        let got = brands
            .logo_for_with(&headers, async move |_domain, _now| {
                counted.fetch_add(1, Ordering::SeqCst);
                None
            })
            .await;
        assert_eq!(got, None);
        assert_eq!(calls.load(Ordering::SeqCst), 0, "DNS lookup skipped");

        // With an expired record whose logo and certificate are already validated on disk, DNS
        // lookup is still skipped while the cached logo continues to serve.
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><rect width="10" height="10" fill="#123456"/></svg>"##;
        let png = draw(svg).expect("draws logo");
        let logo_url = "https://brand.example/logo.svg";
        let cert_url = "https://brand.example/vmc.pem";
        crate::people::save(&brands.file(logo_url), &png).expect("saves logo");
        crate::people::save(&brands.file(cert_url), b"cached-pem").expect("saves cert");
        let expired_record = BrandRecord {
            next: Timestamp::UNIX_EPOCH,
            logo: Some(logo_url.to_owned()),
            certificate: Some(cert_url.to_owned()),
            checked: Some(BrandCheck::Valid {
                until: Timestamp::now() + jiff::SignedDuration::from_hours(24),
            }),
        };
        store
            .save_brand("brand.example", &expired_record)
            .await
            .expect("saves brand");

        let counted = calls.clone();
        let got = brands
            .logo_for_with(&headers, async move |_domain, _now| {
                counted.fetch_add(1, Ordering::SeqCst);
                None
            })
            .await;
        assert_eq!(got, Some(png));
        assert_eq!(calls.load(Ordering::SeqCst), 0, "DNS lookup still skipped");

        // Once remote content blocking is relaxed, an expired record performs the DNS lookup.
        settings.remote_content_blocking = Some(RemoteContentBlocking::ReduceTracking);
        store
            .save_settings(&settings)
            .await
            .expect("saves settings");
        let counted = calls.clone();
        let _ = brands
            .logo_for_with(&headers, async move |_domain, _now| {
                counted.fetch_add(1, Ordering::SeqCst);
                None
            })
            .await;
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "DNS lookup runs when allowed"
        );

        drop(brands);
        drop(store);
        let _ = std::fs::remove_dir_all(&root);
    }
}
