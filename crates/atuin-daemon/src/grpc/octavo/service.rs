use std::sync::Arc;

use atuin_client::ai_session::HarnessSession;
use atuin_octavo::queue::{DirShares, OrgShare, ShareState};
use tonic::{Request, Response, Status};

use super::pb;
use super::pb::threads_server::Threads;
use crate::octavo::{Octavo, ThreadShares, ThreadSharesError};

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
}

fn thread_of(request: Request<pb::ThreadRequest>) -> Result<HarnessSession, Status> {
    let Some(thread) = request.into_inner().thread else {
        return Err(Status::invalid_argument("the request names no thread"));
    };
    Ok(thread.try_into()?)
}

impl From<ThreadSharesError> for Status {
    fn from(err: ThreadSharesError) -> Self {
        Self::internal(err.to_string())
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
