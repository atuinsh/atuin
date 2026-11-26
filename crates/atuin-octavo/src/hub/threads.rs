//! Hub calls for a user's own threads on Octavo: lookup, their shares and links, and labels.

use tonic::Code;
use url::Url;
use uuid::Uuid;

use super::{HubCallError, HubLogin};
use crate::pb;

/// Colors the hub's web app gives a new label (`colorFor` in
/// `web/apps/hub/src/models/label-colors.ts`), so a label made here matches one made there.
const LABEL_COLORS: [&str; 12] = [
    "da5f8b", "e06351", "d37300", "b58800", "839c00", "2ea957", "00ad93", "00a7c4", "0099e6",
    "6a87f0", "9f75e1", "c467bd",
];

/// A share's link; anyone holding it can open the share.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareLink {
    share: Uuid,
    token: String,
}

impl ShareLink {
    fn of(share: &pb::Share) -> Option<Self> {
        let token = share.management.as_ref()?.link_token.clone()?;
        let share = Uuid::from_slice(&share.id.as_ref()?.value).ok()?;
        Some(Self { share, token })
    }

    /// Build the link's URL on the web app at `hub`, as `https://hub.atuin.sh/s/<share>#<token>`.
    pub fn url(&self, hub: &Url) -> Result<Url, url::ParseError> {
        let mut url = hub.join(&format!("s/{}", self.share))?;
        url.set_fragment(Some(&self.token));
        Ok(url)
    }
}

/// What looking for a thread among the user's found.
#[derive(Debug)]
pub enum ThreadLookup {
    Found(Box<pb::Thread>),
    /// Every thread of the harness was read, and none matches.
    NotStored,
    /// None of the pages read holds it; the token of the next page to read.
    NotFoundYet(String),
}

impl HubLogin {
    /// Find the user's thread matching `session` among the 5000 most recently active of its
    /// harness.
    pub async fn find_thread(
        &self,
        session: &pb::HarnessSession,
    ) -> Result<ThreadLookup, HubCallError> {
        /// How many pages are read: the thread a user acts on is one they're working in.
        const PAGES: usize = 25;

        let mut page_token = String::new();
        for _ in 0..PAGES {
            match self.find_thread_in_page(session, page_token).await? {
                ThreadLookup::NotFoundYet(next) => page_token = next,
                found @ (ThreadLookup::Found(_) | ThreadLookup::NotStored) => return Ok(found),
            }
        }
        Ok(ThreadLookup::NotFoundYet(page_token))
    }

    /// Look for the user's thread matching `session` in one page of the threads of its harness,
    /// the most recently active first. An empty `page_token` reads the first page.
    pub async fn find_thread_in_page(
        &self,
        session: &pb::HarnessSession,
        page_token: String,
    ) -> Result<ThreadLookup, HubCallError> {
        const PAGE_SIZE: u32 = 200;

        let request = self.caller.request(pb::ListThreadsRequest {
            filter: Some(pb::ThreadFilter {
                include_subagents: true,
                harness: Some(session.harness.clone()),
                ..pb::ThreadFilter::default()
            }),
            page_size: PAGE_SIZE,
            page_token,
            share: None,
        });
        let page = self
            .caller
            .service
            .clone()
            .list_threads(request)
            .await
            .map_err(HubCallError::from)?
            .into_inner();
        if let Some(thread) =
            page.threads.into_iter().find(|thread| thread.session.as_ref() == Some(session))
        {
            return Ok(ThreadLookup::Found(Box::new(thread)));
        }
        if page.next_page_token.is_empty() {
            return Ok(ThreadLookup::NotStored);
        }
        Ok(ThreadLookup::NotFoundYet(page.next_page_token))
    }

    /// Get a link that opens `thread` to anyone holding it, creating a link-only share if no
    /// share of it has one.
    pub async fn thread_link(&self, thread: &pb::UuidV7) -> Result<ShareLink, HubCallError> {
        if let Some(link) = self.thread_shares(thread).await?.iter().find_map(ShareLink::of) {
            return Ok(link);
        }

        // A link is never added to a share the thread has: setting a share's audience restates its
        // organizations, which would open it again to one closed since they were read.
        let request = self.caller.request(pb::CreateShareRequest {
            id: Some(pb::UuidV7::mint()),
            target: Some(pb::ShareTarget::of_thread(thread)),
            audience: Some(pb::ShareAudience {
                link: true,
                org_slugs: Vec::new(),
            }),
        });
        let created = self
            .caller
            .service
            .clone()
            .create_share(request)
            .await
            .map_err(HubCallError::from)?
            .into_inner();
        created.share.as_ref().and_then(ShareLink::of).ok_or(HubCallError::Incomplete("share link"))
    }

    /// Delete every share of `thread`, so that no link and no organization opens it.
    pub async fn delete_thread_shares(&self, thread: &pb::Thread) -> Result<(), HubCallError> {
        let id = thread.id.clone().ok_or(HubCallError::Incomplete("thread id"))?;
        for share in self.thread_shares(&id).await? {
            let id = share.id.ok_or(HubCallError::Incomplete("share id"))?;
            let request = self.caller.request(pb::DeleteShareRequest { id: Some(id) });
            match self.caller.service.clone().delete_share(request).await {
                Ok(_response) => {}
                // Deleted since it was listed.
                Err(status) if status.code() == Code::NotFound => {}
                Err(status) => return Err(status.into()),
            }
        }
        Ok(())
    }

    /// List every share of `thread`, newest first.
    async fn thread_shares(&self, thread: &pb::UuidV7) -> Result<Vec<pb::Share>, HubCallError> {
        let target = pb::ShareTarget::of_thread(thread);
        let mut shares = Vec::new();
        let mut page_token = String::new();
        loop {
            let request = self.caller.request(pb::ListSharesRequest {
                target: Some(target.clone()),
                page_size: 0,
                page_token,
            });
            let page = self
                .caller
                .service
                .clone()
                .list_shares(request)
                .await
                .map_err(HubCallError::from)?
                .into_inner();
            shares.extend(page.shares);
            if page.next_page_token.is_empty() {
                return Ok(shares);
            }
            page_token = page.next_page_token;
        }
    }

    /// Add the labels named `names` to `thread`, creating any the user lacks, and return the
    /// names of all its labels.
    pub async fn add_labels(
        &self,
        thread: &pb::Thread,
        names: &[String],
    ) -> Result<Vec<String>, HubCallError> {
        let id = thread.id.clone().ok_or(HubCallError::Incomplete("thread id"))?;
        let mut labels = self.labels().await?;
        let mut ids = thread.label_ids.clone();
        let mut missing: Vec<&str> = Vec::new();
        for name in names {
            let Some(label) = labels.iter().find(|label| label.is_named(name)) else {
                if !missing.iter().any(|other| other.to_lowercase() == name.to_lowercase()) {
                    missing.push(name);
                }
                continue;
            };
            let label_id = label.id.clone().ok_or(HubCallError::Incomplete("label id"))?;
            if !ids.contains(&label_id) {
                ids.push(label_id);
            }
        }

        // Checked before any label is created, so a call the hub would refuse leaves none behind.
        let count = ids.len() + missing.len();
        if count > pb::SetThreadLabelsRequest::MAX_LABELS {
            return Err(HubCallError::TooManyLabels {
                count,
                max: pb::SetThreadLabelsRequest::MAX_LABELS,
            });
        }
        for name in missing {
            let created = self.create_label(name).await?;
            let label_id = created.id.clone().ok_or(HubCallError::Incomplete("label id"))?;
            if !ids.contains(&label_id) {
                ids.push(label_id);
            }
            labels.push(created);
        }

        let request = self.caller.request(pb::SetThreadLabelsRequest {
            thread_id: Some(id),
            label_ids: ids.clone(),
        });
        self.caller.service.clone().set_thread_labels(request).await.map_err(HubCallError::from)?;
        Ok(labels
            .into_iter()
            .filter(|label| label.id.as_ref().is_some_and(|id| ids.contains(id)))
            .map(|label| label.name)
            .collect())
    }

    async fn labels(&self) -> Result<Vec<pb::Label>, HubCallError> {
        let request = self.caller.request(pb::ListLabelsRequest {});
        let labels =
            self.caller.service.clone().list_labels(request).await.map_err(HubCallError::from)?;
        Ok(labels.into_inner().labels)
    }

    async fn create_label(&self, name: &str) -> Result<pb::Label, HubCallError> {
        let request = self.caller.request(pb::CreateLabelRequest {
            label: Some(pb::Label {
                id: Some(pb::UuidV7::mint()),
                name: name.to_owned(),
                color: pb::Label::color_for(name).to_owned(),
                description: String::new(),
            }),
        });
        match self.caller.service.clone().create_label(request).await {
            Ok(created) => created.into_inner().label.ok_or(HubCallError::Incomplete("label")),
            Err(status) if status.code() == Code::InvalidArgument => {
                Err(HubCallError::InvalidLabel(name.to_owned(), status))
            }
            // Another call created it since the labels were listed.
            Err(status) if status.code() == Code::AlreadyExists => self
                .labels()
                .await?
                .into_iter()
                .find(|label| label.is_named(name))
                .ok_or_else(|| status.into()),
            Err(status) => Err(status.into()),
        }
    }
}

impl pb::ShareTarget {
    fn of_thread(thread: &pb::UuidV7) -> Self {
        Self {
            target: Some(pb::share_target::Target::Thread(pb::SharedThread {
                id: Some(thread.clone()),
            })),
        }
    }
}

impl pb::SetThreadLabelsRequest {
    /// The `max_items` of `label_ids` in `hub_service.proto`.
    const MAX_LABELS: usize = 20;
}

impl pb::Label {
    /// Whether this label has `name`, ignoring case, as the hub compares label names.
    fn is_named(&self, name: &str) -> bool {
        self.name.to_lowercase() == name.to_lowercase()
    }

    /// Pick a new label's color from [`LABEL_COLORS`] by the FNV-1a hash of its lowercase name.
    fn color_for(name: &str) -> &'static str {
        const OFFSET_BASIS: u32 = 0x811c_9dc5;
        const PRIME: u32 = 0x0100_0193;

        let hash = name
            .to_lowercase()
            .chars()
            .fold(OFFSET_BASIS, |hash, char| (hash ^ u32::from(char)).wrapping_mul(PRIME));
        LABEL_COLORS[hash as usize % LABEL_COLORS.len()]
    }
}
