// SPDX-License-Identifier: MIT OR Apache-2.0
//! Optional passive sinks must not advertise an absent subscriber.
use tracing::{
    Dispatch, Event, Subscriber,
    span::{Attributes, Id, Record},
};
use tracing_subscriber::{Layer, layer::Context};

// Option<Layer>'s None downcast marker propagates through Layered. At OFF,
// a later passive layer with no hint can then override OFF with None (TRACE).
// Keep optionality opaque: only the root reloadable EnvFilter controls interest
// and the maximum-level hint. This wrapper adds no allocation or worker.
pub(crate) struct Sink<L>(pub(crate) Option<L>);

/// Keep events already printed by failure() out of stderr, without changing
/// subscriber interest or filtering the journal, file and recent-event sinks.
pub(crate) struct Stderr<L>(pub(crate) L);

impl<S: Subscriber, L: Layer<S>> Layer<S> for Stderr<L> {
    fn on_layer(&mut self, subscriber: &mut S) {
        self.0.on_layer(subscriber);
    }
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        self.0.on_new_span(attrs, id, ctx);
    }
    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        self.0.on_record(id, values, ctx);
    }
    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        if event.metadata().target() != crate::debug::FAILURE_TARGET {
            self.0.on_event(event, ctx);
        }
    }
    fn on_enter(&self, id: &Id, ctx: Context<'_, S>) {
        self.0.on_enter(id, ctx);
    }
    fn on_exit(&self, id: &Id, ctx: Context<'_, S>) {
        self.0.on_exit(id, ctx);
    }
    fn on_close(&self, id: Id, ctx: Context<'_, S>) {
        self.0.on_close(id, ctx);
    }
}

impl<S: Subscriber, L: Layer<S>> Layer<S> for Sink<L> {
    fn on_register_dispatch(&self, dispatch: &Dispatch) {
        if let Some(layer) = &self.0 {
            layer.on_register_dispatch(dispatch);
        }
    }
    fn on_layer(&mut self, subscriber: &mut S) {
        if let Some(layer) = &mut self.0 {
            layer.on_layer(subscriber);
        }
    }
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        if let Some(layer) = &self.0 {
            layer.on_new_span(attrs, id, ctx);
        }
    }
    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        if let Some(layer) = &self.0 {
            layer.on_record(id, values, ctx);
        }
    }
    fn on_follows_from(&self, id: &Id, follows: &Id, ctx: Context<'_, S>) {
        if let Some(layer) = &self.0 {
            layer.on_follows_from(id, follows, ctx);
        }
    }
    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        if let Some(layer) = &self.0 {
            layer.on_event(event, ctx);
        }
    }
    fn on_enter(&self, id: &Id, ctx: Context<'_, S>) {
        if let Some(layer) = &self.0 {
            layer.on_enter(id, ctx);
        }
    }
    fn on_exit(&self, id: &Id, ctx: Context<'_, S>) {
        if let Some(layer) = &self.0 {
            layer.on_exit(id, ctx);
        }
    }
    fn on_close(&self, id: Id, ctx: Context<'_, S>) {
        if let Some(layer) = &self.0 {
            layer.on_close(id, ctx);
        }
    }
    fn on_id_change(&self, old: &Id, new: &Id, ctx: Context<'_, S>) {
        if let Some(layer) = &self.0 {
            layer.on_id_change(old, new, ctx);
        }
    }
}
