//! Core daemon infrastructure.
//!
//! This module provides the foundational types for building the atuin daemon:
//!
//! - [`DaemonState`]: Shared state owned by the daemon
//! - [`DaemonHandle`]: A lightweight, cloneable handle for accessing daemon state
//! - [`Component`]: A trait for implementing daemon components
//! - [`Daemon`]: The main daemon orchestrator
//! - [`DaemonBuilder`]: Builder for constructing and configuring the daemon

use std::sync::Arc;

use atuin_client::{
    database::Sqlite as HistoryDatabase, encryption, record::sqlite_store::SqliteStore,
    settings::Settings,
};
use eyre::{Context, Result};
use tokio::sync::{RwLock, broadcast};

use crate::events::DaemonEvent;

// ============================================================================
// DaemonState
// ============================================================================

/// Shared state owned by the daemon.
///
/// This contains all the resources that components and services need access to.
/// The state is wrapped in an `Arc` and accessed via [`DaemonHandle`].
pub struct DaemonState {
    // Event bus
    event_tx: broadcast::Sender<DaemonEvent>,

    // Configuration (mutable - can be reloaded)
    settings: RwLock<Settings>,

    // Encryption key (immutable - derived at startup)
    encryption_key: [u8; 32],

    // Database handles
    history_db: HistoryDatabase,
    store: SqliteStore,
}

// ============================================================================
// DaemonHandle
// ============================================================================

/// A lightweight handle to the daemon's shared state.
///
/// This is the primary way for components, gRPC services, and spawned tasks to
/// interact with the daemon. It provides access to:
///
/// - Event emission and subscription
/// - Configuration (settings, encryption key)
/// - Database handles
///
/// The handle is cheaply cloneable (wraps an `Arc`) and can be freely passed
/// around to any code that needs daemon access.
///
/// # Example
///
/// ```ignore
/// // Emit an event
/// handle.emit(DaemonEvent::HistoryPruned);
///
/// // Access settings
/// let settings = handle.settings().await;
/// let sync_freq = settings.daemon.sync_frequency;
///
/// // Access database
/// let history = handle.history_db().load(id).await?;
/// ```
#[derive(Clone)]
pub struct DaemonHandle {
    state: Arc<DaemonState>,
}

impl DaemonHandle {
    // ---- Events ----

    /// Emit an event to the daemon's event bus.
    ///
    /// This is fire-and-forget - if no receivers are listening (which shouldn't
    /// happen in normal operation), the event is dropped silently.
    pub fn emit(&self, event: DaemonEvent) {
        if let Err(e) = self.state.event_tx.send(event) {
            tracing::warn!("failed to emit event (no receivers?): {e}");
        }
    }

    /// Subscribe to the event bus.
    ///
    /// Returns a receiver that will receive all events emitted after this call.
    /// Useful for components that need to listen for events outside of the
    /// normal `handle_event` callback flow.
    pub fn subscribe(&self) -> broadcast::Receiver<DaemonEvent> {
        self.state.event_tx.subscribe()
    }

    /// Request graceful shutdown of the daemon.
    pub fn shutdown(&self) {
        self.emit(DaemonEvent::ShutdownRequested);
    }

    // ---- Configuration ----

    /// Get the current settings.
    ///
    /// This acquires a read lock on the settings. For most use cases, clone
    /// the settings if you need to hold onto them.
    pub async fn settings(&self) -> tokio::sync::RwLockReadGuard<'_, Settings> {
        self.state.settings.read().await
    }

    /// Reload settings from disk and emit a SettingsReloaded event.
    ///
    /// Components listening for `SettingsReloaded` can then re-read settings
    /// via `handle.settings()` to pick up the changes.
    pub async fn reload_settings(&self) -> Result<()> {
        let new_settings = Settings::new()?;
        *self.state.settings.write().await = new_settings;
        self.emit(DaemonEvent::SettingsReloaded);
        tracing::info!("settings reloaded");
        Ok(())
    }

    /// Get the encryption key.
    pub fn encryption_key(&self) -> &[u8; 32] {
        &self.state.encryption_key
    }

    // ---- Database ----

    /// Get a reference to the history database.
    pub fn history_db(&self) -> &HistoryDatabase {
        &self.state.history_db
    }

    /// Get a reference to the record store.
    pub fn store(&self) -> &SqliteStore {
        &self.state.store
    }
}

impl std::fmt::Debug for DaemonHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DaemonHandle").finish_non_exhaustive()
    }
}

// ============================================================================
// Component Trait
// ============================================================================

/// A daemon component that handles a specific domain.
///
/// Components are the building blocks of the daemon. Each component:
///
/// - Has a unique name for logging and debugging
/// - Can optionally expose gRPC services
/// - Receives a [`DaemonHandle`] on startup for accessing daemon resources
/// - Handles events from the event bus
/// - Performs cleanup on shutdown
///
/// # Lifecycle
///
/// 1. **Construction**: Component is created (usually via `new()`)
/// 2. **Start**: `start()` is called with a [`DaemonHandle`]
/// 3. **Running**: `handle_event()` is called for each event on the bus
/// 4. **Shutdown**: `stop()` is called for cleanup
///
/// # Example
///
/// ```ignore
/// pub struct MyComponent {
///     handle: Option<DaemonHandle>,
/// }
///
/// #[async_trait]
/// impl Component for MyComponent {
///     fn name(&self) -> &'static str { "my-component" }
///
///     async fn start(&mut self, handle: DaemonHandle) -> Result<()> {
///         self.handle = Some(handle);
///         Ok(())
///     }
///
///     async fn handle_event(&mut self, event: &DaemonEvent) -> Result<()> {
///         match event {
///             DaemonEvent::SomeEvent => {
///                 // Handle the event
///                 if let Some(handle) = &self.handle {
///                     handle.emit(DaemonEvent::ResponseEvent);
///                 }
///             }
///             _ => {}
///         }
///         Ok(())
///     }
///
///     async fn stop(&mut self) -> Result<()> {
///         Ok(())
///     }
/// }
/// ```
#[tonic::async_trait]
pub trait Component: Send + Sync {
    /// Human-readable name for logging and debugging.
    fn name(&self) -> &'static str;

    /// Called once at startup.
    ///
    /// Store the handle if you need to emit events or access daemon resources
    /// later. The handle is cheaply cloneable, so feel free to clone it for
    /// spawned tasks.
    async fn start(&mut self, handle: DaemonHandle) -> Result<()>;

    /// Handle an incoming event.
    ///
    /// Called for every event on the bus. To emit new events in response,
    /// use the handle stored during `start()`. Events emitted here will be
    /// processed in subsequent event loop iterations.
    async fn handle_event(&mut self, event: &DaemonEvent) -> Result<()>;

    /// Called on graceful shutdown.
    ///
    /// Use this to clean up resources, abort spawned tasks, etc.
    async fn stop(&mut self) -> Result<()>;
}

// ============================================================================
// Daemon
// ============================================================================

/// The main daemon orchestrator.
///
/// The daemon manages components, runs the event loop, and coordinates startup
/// and shutdown. It is constructed via [`DaemonBuilder`].
///
/// # Event Loop
///
/// The daemon runs a simple event loop:
///
/// 1. Wait for an event on the bus
/// 2. Dispatch the event to all components (in registration order)
/// 3. Components may emit new events in response
/// 4. Repeat until `ShutdownRequested` is received
///
/// Events emitted during handling are queued and processed in subsequent
/// iterations, ensuring the loop eventually drains.
pub struct Daemon {
    components: Vec<Box<dyn Component>>,
    handle: DaemonHandle,
}

impl Daemon {
    /// Create a new daemon builder.
    pub fn builder(settings: Settings) -> DaemonBuilder {
        DaemonBuilder::new(settings)
    }

    /// Get a clone of the daemon handle.
    ///
    /// The handle can be used to emit events, access settings, etc.
    pub fn handle(&self) -> DaemonHandle {
        self.handle.clone()
    }

    /// Start all components.
    ///
    /// This must be called before `run_event_loop()`. It initializes all
    /// registered components with the daemon handle.
    pub async fn start_components(&mut self) -> Result<()> {
        for component in &mut self.components {
            tracing::info!(component = component.name(), "starting component");
            component
                .start(self.handle.clone())
                .await
                .with_context(|| format!("failed to start component: {}", component.name()))?;
        }
        Ok(())
    }

    /// Run the daemon event loop.
    ///
    /// This processes events until a ShutdownRequested event is received.
    /// Components must be started first via `start_components()`.
    pub async fn run_event_loop(&mut self) -> Result<()> {
        let mut event_rx = self.handle.subscribe();
        loop {
            match event_rx.recv().await {
                Ok(DaemonEvent::ShutdownRequested) => {
                    tracing::info!("shutdown requested, stopping daemon");
                    break;
                }
                Ok(event) => {
                    tracing::debug!(?event, "processing event");
                    self.dispatch_event(&event).await;
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(
                        skipped = n,
                        "event receiver lagged, some events were dropped"
                    );
                }
                Err(broadcast::error::RecvError::Closed) => {
                    tracing::info!("event bus closed, stopping daemon");
                    break;
                }
            }
        }
        Ok(())
    }

    /// Stop all components.
    ///
    /// This performs graceful shutdown of all components.
    pub async fn stop_components(&mut self) {
        for component in &mut self.components {
            tracing::info!(component = component.name(), "stopping component");
            if let Err(e) = component.stop().await {
                tracing::error!(
                    component = component.name(),
                    error = ?e,
                    "error stopping component"
                );
            }
        }
        tracing::info!("all components stopped");
    }

    /// Run the daemon.
    ///
    /// This is a convenience method that starts components, runs the event loop,
    /// and handles shutdown. It does not return until the daemon is shut down.
    pub async fn run(mut self) -> Result<()> {
        self.start_components().await?;
        self.run_event_loop().await?;
        self.stop_components().await;
        tracing::info!("daemon stopped");
        Ok(())
    }

    async fn dispatch_event(&mut self, event: &DaemonEvent) {
        for component in &mut self.components {
            if let Err(e) = component.handle_event(event).await {
                tracing::error!(
                    component = component.name(),
                    error = ?e,
                    "error handling event"
                );
            }
        }
    }
}

// ============================================================================
// DaemonBuilder
// ============================================================================

/// Builder for constructing a [`Daemon`].
///
/// # Example
///
/// ```ignore
/// let daemon = Daemon::builder(settings)
///     .store(store)
///     .history_db(history_db)
///     .component(HistoryComponent::new())
///     .component(SearchComponent::new())
///     .component(SyncComponent::new())
///     .build()
///     .await?;
///
/// daemon.run().await?;
/// ```
pub struct DaemonBuilder {
    settings: Settings,
    store: Option<SqliteStore>,
    history_db: Option<HistoryDatabase>,
    components: Vec<Box<dyn Component>>,
}

impl DaemonBuilder {
    /// Create a new daemon builder with the given settings.
    pub fn new(settings: Settings) -> Self {
        Self {
            settings,
            store: None,
            history_db: None,
            components: Vec::new(),
        }
    }

    /// Set the record store.
    pub fn store(mut self, store: SqliteStore) -> Self {
        self.store = Some(store);
        self
    }

    /// Set the history database.
    pub fn history_db(mut self, db: HistoryDatabase) -> Self {
        self.history_db = Some(db);
        self
    }

    /// Register a component.
    ///
    /// Components are started in registration order and stopped in reverse order.
    pub fn component(mut self, component: impl Component + 'static) -> Self {
        self.components.push(Box::new(component));
        self
    }

    /// Build the daemon.
    ///
    /// This loads the encryption key and creates the daemon state.
    pub async fn build(self) -> Result<Daemon> {
        let store = self.store.ok_or_else(|| eyre::eyre!("store is required"))?;
        let history_db = self
            .history_db
            .ok_or_else(|| eyre::eyre!("history_db is required"))?;

        // Load encryption key
        let encryption_key: [u8; 32] = encryption::load_key(&self.settings)
            .context("could not load encryption key")?
            .into();

        // Create the event bus
        let (event_tx, _) = broadcast::channel(64);

        // Create the shared state
        let state = Arc::new(DaemonState {
            event_tx,
            settings: RwLock::new(self.settings),
            encryption_key,
            history_db,
            store,
        });

        // Create the handle (just a reference to the state)
        let handle = DaemonHandle { state };

        Ok(Daemon {
            components: self.components,
            handle,
        })
    }
}
