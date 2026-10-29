use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
pub(crate) struct LogCapture(Arc<Mutex<Vec<CapturedEvent>>>);

#[derive(Clone, Debug)]
pub(crate) struct CapturedEvent {
    pub fields: BTreeMap<String, String>,
    pub level: tracing::Level,
}

impl LogCapture {
    pub(crate) fn events(&self, name: &str) -> Vec<CapturedEvent> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|event| {
                event
                    .fields
                    .get("event")
                    .is_some_and(|value| value == &format!("{name:?}"))
            })
            .cloned()
            .collect()
    }

    pub(crate) fn text(&self) -> String {
        format!("{:?}", self.0.lock().unwrap())
    }
}

impl tracing::Subscriber for LogCapture {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        struct Fields(BTreeMap<String, String>);
        impl tracing::field::Visit for Fields {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                self.0.insert(field.name().into(), format!("{value:?}"));
            }
        }
        let mut fields = Fields(BTreeMap::new());
        event.record(&mut fields);
        self.0.lock().unwrap().push(CapturedEvent {
            fields: fields.0,
            level: *event.metadata().level(),
        });
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}
