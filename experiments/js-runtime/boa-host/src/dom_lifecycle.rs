//! A deliberately small DOM-like native wrapper lifecycle experiment.
//!
//! This is not a DOM implementation. It tests the ownership boundary between
//! Boa JavaScript objects, native Rust wrapper data, and a Rust-side node graph.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use boa_engine::class::{Class, ClassBuilder};
use boa_engine::gc::{Gc, GcRefCell};
use boa_engine::native_function::NativeFunction;
use boa_engine::{Context, JsArgs, JsData, JsNativeError, JsResult, JsValue, Source, js_string};
use boa_gc::{Finalize, Trace, force_collect};

static FINALIZED_WRAPPERS: AtomicUsize = AtomicUsize::new(0);
static FINALIZED_STATES: AtomicUsize = AtomicUsize::new(0);
static TEST_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Trace)]
struct NodeState {
    children: Vec<Gc<GcRefCell<NodeState>>>,
}

impl Finalize for NodeState {
    fn finalize(&self) {
        FINALIZED_STATES.fetch_add(1, Ordering::SeqCst);
    }
}

#[derive(Debug, Trace, JsData)]
struct DomNode {
    state: Gc<GcRefCell<NodeState>>,
}

impl Finalize for DomNode {
    fn finalize(&self) {
        FINALIZED_WRAPPERS.fetch_add(1, Ordering::SeqCst);
    }
}

impl Class for DomNode {
    const NAME: &'static str = "DomNode";

    fn data_constructor(
        _new_target: &JsValue,
        _args: &[JsValue],
        _context: &mut Context,
    ) -> JsResult<Self> {
        Ok(Self {
            state: Gc::new(GcRefCell::new(NodeState {
                children: Vec::new(),
            })),
        })
    }

    fn init(class: &mut ClassBuilder<'_>) -> JsResult<()> {
        class.method(
            js_string!("appendChild"),
            1,
            NativeFunction::from_fn_ptr(|this, args, _context| {
                let parent_state = {
                    let object = this.as_object().ok_or_else(|| {
                        JsNativeError::typ().with_message("appendChild receiver is not an object")
                    })?;
                    let parent = object.downcast_ref::<DomNode>().ok_or_else(|| {
                        JsNativeError::typ().with_message("appendChild receiver is not a DomNode")
                    })?;
                    parent.state.clone()
                };

                let child_object = args
                    .get_or_undefined(0)
                    .as_object()
                    .ok_or_else(|| JsNativeError::typ().with_message("child is not an object"))?;
                let child_state = {
                    let child = child_object.downcast_ref::<DomNode>().ok_or_else(|| {
                        JsNativeError::typ().with_message("child is not a DomNode")
                    })?;
                    child.state.clone()
                };

                parent_state.borrow_mut().children.push(child_state);
                Ok(child_object.into())
            }),
        );
        class.method(
            js_string!("childCount"),
            0,
            NativeFunction::from_fn_ptr(|this, _args, _context| {
                let object = this.as_object().ok_or_else(|| {
                    JsNativeError::typ().with_message("childCount receiver is not an object")
                })?;
                let node = object.downcast_ref::<DomNode>().ok_or_else(|| {
                    JsNativeError::typ().with_message("childCount receiver is not a DomNode")
                })?;
                let count = node.state.borrow().children.len() as u32;
                Ok(JsValue::from(count))
            }),
        );
        Ok(())
    }
}

#[test]
fn native_wrapper_exposes_a_minimal_node_graph() {
    let _guard = TEST_LOCK.lock().expect("test lock");
    let mut context = Context::default();
    context
        .register_global_class::<DomNode>()
        .expect("native class registers");

    let value = context
        .eval(Source::from_bytes(
            "const parent = new DomNode(); const child = new DomNode(); parent.appendChild(child); parent.childCount()",
        ))
        .expect("native wrapper graph works");
    assert_eq!(value.display().to_string(), "1");
}

#[test]
fn unrooted_js_and_native_node_cycles_are_finalized() {
    let _guard = TEST_LOCK.lock().expect("test lock");
    let wrappers_before = FINALIZED_WRAPPERS.load(Ordering::SeqCst);
    let states_before = FINALIZED_STATES.load(Ordering::SeqCst);
    let mut context = Context::default();
    context
        .register_global_class::<DomNode>()
        .expect("native class registers");

    context
        .eval(Source::from_bytes(
            "globalThis.parent = new DomNode(); globalThis.child = new DomNode(); parent.appendChild(child); child.appendChild(parent); parent.self = parent; child.self = child; delete globalThis.parent; delete globalThis.child;",
        ))
        .expect("cross-linked wrappers are created and unrooted");

    for _ in 0..3 {
        force_collect();
    }

    assert!(
        FINALIZED_WRAPPERS.load(Ordering::SeqCst) >= wrappers_before + 2,
        "unreachable native wrappers should be finalized"
    );
    assert!(
        FINALIZED_STATES.load(Ordering::SeqCst) >= states_before + 2,
        "unreachable native node graph cycles should be finalized"
    );
}
