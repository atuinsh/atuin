//! Hub calls for a user's own threads on Octavo: lookup and their shares.

use tonic::Code;

use super::{HubCallError, HubLogin};
use crate::pb;

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
        let target = pb::ShareTarget {
            target: Some(pb::share_target::Target::Thread(pb::SharedThread {
                id: Some(thread.clone()),
            })),
        };
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
}
