pub struct LayerId(pub u64);

pub trait Layer {
    fn id(&self) -> LayerId;
}
