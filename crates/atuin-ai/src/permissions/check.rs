use eyre::Result;

pub(crate) struct PermissionRequest {
    call: ToolCall,
}

pub(crate) enum PermissionResponse {
    Allowed,
    Denied,
    Ask,
}

pub(crate) struct PermissionsChecker {
    //
}

impl PermissionsChecker {
    pub fn new() -> Self {
        Self {}
    }

    pub async fn check(&self, request: &PermissionRequest) -> Result<PermissionResponse> {
        //
    }
}
