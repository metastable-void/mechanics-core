use super::builtins;
use crate::executor::CustomModuleLoader;
use boa_engine::Context;
use std::rc::Rc;

pub(super) fn install_synthetic_modules(loader: &Rc<CustomModuleLoader>, context: &mut Context) {
    builtins::bundle_builtin_modules(loader, context);
}
