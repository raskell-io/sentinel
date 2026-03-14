//! GatewayClass reconciler.
//!
//! Watches GatewayClass resources and claims ownership for the `zentinel`
//! controller name. Sets the `Accepted` condition on GatewayClasses that
//! match our controller name.

use gateway_api::gatewayclasses::GatewayClass;
use kube::api::{Api, Patch, PatchParams};
use kube::runtime::controller::Action;
use kube::{Client, ResourceExt};
use serde_json::json;
use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::error::GatewayError;

/// The controller name that identifies Zentinel as the implementation.
pub const CONTROLLER_NAME: &str = "zentinelproxy.io/gateway-controller";

/// Reconciler for GatewayClass resources.
pub struct GatewayClassReconciler {
    client: Client,
}

impl GatewayClassReconciler {
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    /// Reconcile a GatewayClass resource.
    ///
    /// If the GatewayClass references our controller name, we accept it
    /// by setting the `Accepted` status condition.
    pub async fn reconcile(
        &self,
        gateway_class: Arc<GatewayClass>,
    ) -> Result<Action, GatewayError> {
        let name = gateway_class.name_any();
        let controller_name = &gateway_class.spec.controller_name;

        if controller_name != CONTROLLER_NAME {
            debug!(
                name = %name,
                controller = %controller_name,
                "Ignoring GatewayClass for different controller"
            );
            return Ok(Action::await_change());
        }

        info!(name = %name, "Accepting GatewayClass");

        let api: Api<GatewayClass> = Api::all(self.client.clone());
        let now = chrono::Utc::now().to_rfc3339();
        let status = json!({
            "status": {
                "conditions": [{
                    "type": "Accepted",
                    "status": "True",
                    "reason": "Accepted",
                    "message": "GatewayClass accepted by Zentinel controller",
                    "observedGeneration": gateway_class.metadata.generation.unwrap_or(0),
                    "lastTransitionTime": now,
                }]
            }
        });

        api.patch_status(
            &name,
            &PatchParams::apply(CONTROLLER_NAME),
            &Patch::Merge(&status),
        )
        .await?;

        Ok(Action::await_change())
    }

    /// Handle errors during reconciliation.
    pub fn error_policy(
        _obj: Arc<GatewayClass>,
        error: &GatewayError,
        _ctx: Arc<()>,
    ) -> Action {
        warn!(error = %error, "GatewayClass reconciliation failed");
        Action::requeue(std::time::Duration::from_secs(30))
    }
}
