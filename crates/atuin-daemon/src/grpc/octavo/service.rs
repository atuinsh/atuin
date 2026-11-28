use std::path::PathBuf;
use std::sync::Arc;

use atuin_client::ai_session::HarnessSession;
use atuin_octavo::hub::HubCallError;
use atuin_octavo::queue::{DirShares, OrgShare, ShareState};
use tonic::{Request, Response, Status};

use super::pb;
use super::pb::threads_server::Threads;
use crate::octavo::{Octavo, ThreadActionError, ThreadShares, ThreadSharesError};

#[derive(Debug, Clone)]
pub struct Service {
    octavo: Arc<Octavo>,
}

impl Service {
    #[must_use]
    pub const fn new(octavo: Arc<Octavo>) -> Self {
        Self { octavo }
    }
}

#[tonic::async_trait]
impl Threads for Service {
    async fn get_shares(
        &self,
        request: Request<pb::ThreadRequest>,
    ) -> Result<Response<pb::ThreadShares>, Status> {
        let shares = self.octavo.thread_shares(thread_of(request)?).await?;
        Ok(Response::new(shares.into()))
    }

    async fn make_private(
        &self,
        request: Request<pb::ThreadRequest>,
    ) -> Result<Response<pb::ThreadShares>, Status> {
        let shares = self.octavo.make_private(thread_of(request)?).await?;
        Ok(Response::new(shares.into()))
    }

    async fn share_link(
        &self,
        request: Request<pb::ThreadRequest>,
    ) -> Result<Response<pb::ShareLinkReply>, Status> {
        let url = self.octavo.share_link(thread_of(request)?).await?;
        Ok(Response::new(pb::ShareLinkReply { url: url.into() }))
    }

    async fn add_labels(
        &self,
        request: Request<pb::AddLabelsRequest>,
    ) -> Result<Response<pb::AddLabelsReply>, Status> {
        let pb::AddLabelsRequest { thread, names } = request.into_inner();
        let Some(thread) = thread else {
            return Err(Status::invalid_argument("the request names no thread"));
        };
        let names = self.octavo.add_labels(thread.try_into()?, &names).await?;
        Ok(Response::new(pb::AddLabelsReply { names }))
    }

    async fn thread_started(
        &self,
        request: Request<pb::ThreadStartedRequest>,
    ) -> Result<Response<pb::ThreadStartedReply>, Status> {
        let pb::ThreadStartedRequest { thread, dir } = request.into_inner();
        let Some(thread) = thread else {
            return Err(Status::invalid_argument("the request names no thread"));
        };
        if dir.is_empty() {
            return Err(Status::invalid_argument("the request names no directory"));
        }
        self.octavo.thread_started(thread.try_into()?, PathBuf::from(dir));
        Ok(Response::new(pb::ThreadStartedReply {}))
    }

    async fn get_new_thread_shares(
        &self,
        request: Request<pb::NewThreadRequest>,
    ) -> Result<Response<pb::ThreadShares>, Status> {
        let id = request.into_inner().session_id;
        let Some(shares) = self.octavo.new_thread_shares(&id).await else {
            return Err(Status::not_found(format!("no agent started a thread `{id}` lately")));
        };
        Ok(Response::new(shares.into()))
    }
}

fn thread_of(request: Request<pb::ThreadRequest>) -> Result<HarnessSession, Status> {
    let Some(thread) = request.into_inner().thread else {
        return Err(Status::invalid_argument("the request names no thread"));
    };
    Ok(thread.try_into()?)
}

impl From<ThreadSharesError> for Status {
    fn from(err: ThreadSharesError) -> Self {
        match err {
            ThreadSharesError::NotShareable => Self::invalid_argument(err.to_string()),
            ThreadSharesError::CapturedElsewhere => Self::failed_precondition(err.to_string()),
            ThreadSharesError::Queue(_) => Self::internal(err.to_string()),
        }
    }
}

impl From<ThreadActionError> for Status {
    fn from(err: ThreadActionError) -> Self {
        let message = err.causes();
        match err {
            ThreadActionError::Off
            | ThreadActionError::LoggedOut
            | ThreadActionError::NotStored
            | ThreadActionError::NotAmongRecent
            | ThreadActionError::CapturedElsewhere
            | ThreadActionError::Private => Self::failed_precondition(message),
            ThreadActionError::NotShareable
            | ThreadActionError::Hub(
                HubCallError::InvalidLabel(..) | HubCallError::TooManyLabels { .. },
            ) => Self::invalid_argument(message),
            ThreadActionError::Login(_)
            | ThreadActionError::Hub(_)
            | ThreadActionError::LinkUrl(_) => Self::unavailable(message),
            ThreadActionError::Queue(_) => Self::internal(message),
        }
    }
}

impl From<ThreadShares> for pb::ThreadShares {
    fn from(shares: ThreadShares) -> Self {
        let queued = match shares {
            ThreadShares::Off => return Self::of(pb::Upload::Off),
            ThreadShares::LoggedOut => return Self::of(pb::Upload::LoggedOut),
            ThreadShares::On(queued) => queued,
        };
        Self {
            upload: pb::Upload::On.into(),
            orgs: queued.orgs.into_iter().map(Into::into).collect(),
            dirs: queued.dirs.into_iter().map(Into::into).collect(),
            is_private: queued.is_private,
            revoking_links: queued.revoking_links,
        }
    }
}

impl pb::ThreadShares {
    fn of(upload: pb::Upload) -> Self {
        Self {
            upload: upload.into(),
            ..Self::default()
        }
    }
}

impl From<OrgShare> for pb::OrgShare {
    fn from(share: OrgShare) -> Self {
        let state = match share.state {
            ShareState::Opening => pb::ShareState::Opening,
            ShareState::Open => pb::ShareState::Open,
            ShareState::Closing => pb::ShareState::Closing,
            ShareState::Closed => pb::ShareState::Closed,
        };
        Self {
            org_slug: share.org.to_string(),
            state: state.into(),
        }
    }
}

impl From<DirShares> for pb::DirShares {
    fn from(dir: DirShares) -> Self {
        Self {
            dir: dir.dir.to_string_lossy().into_owned(),
            org_slugs: dir.orgs.iter().map(ToString::to_string).collect(),
        }
    }
}
