//! The backend for platforms that don't have an accessibility backend yet.

use std::convert::Infallible;
use std::time::Duration;

use crate::backend::{
    AppInfo, Backend, Error, Event, EventCount, Inspect, Limits, Platform, Purpose, Reading,
    WindowClass,
};
use crate::outline::Outline;
use crate::readiness::UnreadyApp;

/// A backend that never connects.
pub struct Unsupported(Infallible);

#[expect(clippy::unused_async_trait_impl, reason = "the trait methods are async")]
impl Backend for Unsupported {
    type App = Infallible;
    type Window = Infallible;

    async fn connect(_: Purpose) -> Result<Self, Error> {
        Err(Error::Unsupported)
    }

    async fn wake_apps(&self) -> Option<Duration> {
        match self.0 {}
    }

    async fn apps(&self) -> Result<Vec<Infallible>, Error> {
        match self.0 {}
    }

    async fn windows(&self, _: &Infallible) -> Result<Vec<Infallible>, Error> {
        match self.0 {}
    }

    fn app_of(&self, _: &Infallible) -> Infallible {
        match self.0 {}
    }

    fn is_terminal_name(_: &str) -> bool {
        false
    }

    async fn app_name(&self, _: &Infallible) -> Result<String, Error> {
        match self.0 {}
    }

    async fn active_window(&self) -> Option<Infallible> {
        match self.0 {}
    }

    async fn active_window_of(&self, _: &Infallible) -> Option<Infallible> {
        match self.0 {}
    }

    async fn top_level(&self, _: Infallible) -> Infallible {
        match self.0 {}
    }

    async fn window_class(&self, _: &Infallible) -> Result<WindowClass, Error> {
        match self.0 {}
    }

    async fn window_title(&self, _: &Infallible) -> Result<String, Error> {
        match self.0 {}
    }

    async fn read(&self, _: &Infallible, _: Limits) -> Result<Reading, Error> {
        match self.0 {}
    }

    async fn next(&mut self) -> Option<Event<Self>> {
        match self.0 {}
    }

    fn take_event_counts(&mut self) -> impl Iterator<Item = EventCount<Infallible>> + use<> {
        std::iter::empty()
    }
}
impl Platform for Unsupported {}

#[expect(clippy::unused_async_trait_impl, reason = "the trait methods are async")]
impl Inspect for Unsupported {
    async fn unready_apps(&self) -> Result<Vec<UnreadyApp>, Error> {
        match self.0 {}
    }

    async fn app_info(&self, _: &Infallible) -> Result<AppInfo, Error> {
        match self.0 {}
    }

    async fn outline(&self, _: &Infallible, _: usize) -> Result<Outline, Error> {
        match self.0 {}
    }
}
