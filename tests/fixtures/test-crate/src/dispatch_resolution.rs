//! # Dispatch Resolution Test Fixtures
//!
//! Exercises static, trait, and dynamic dispatch for the ingestion pipeline's
//! trait impl call resolution feature.

/// A trait for dispatching work items.
pub trait Dispatcher {
    /// Dispatch a work item.
    fn dispatch(&self, item: &str) -> String;
    /// Check if the dispatcher is ready.
    fn is_ready(&self) -> bool;
}

/// FIFO dispatcher — processes items in order.
pub struct FifoDispatcher {
    pub capacity: usize,
}

impl Dispatcher for FifoDispatcher {
    fn dispatch(&self, item: &str) -> String {
        format!("FIFO: {}", item)
    }
    fn is_ready(&self) -> bool {
        self.capacity > 0
    }
}

/// Priority dispatcher — processes items by priority.
pub struct PriorityDispatcher {
    pub max_priority: u8,
}

impl Dispatcher for PriorityDispatcher {
    fn dispatch(&self, item: &str) -> String {
        format!("PRIORITY({}): {}", self.max_priority, item)
    }
    fn is_ready(&self) -> bool {
        self.max_priority > 0
    }
}

/// Calls trait methods on a CONCRETE type (static dispatch).
/// The resolver should emit a CALLS edge to FifoDispatcher::dispatch (concrete impl FQN)
/// with dispatch="static".
pub fn static_dispatch_call() -> String {
    let dispatcher = FifoDispatcher { capacity: 10 };
    dispatcher.dispatch("hello")
}

/// Calls trait methods on a DYNAMIC trait object (dynamic dispatch).
/// The resolver should emit a CALLS edge to Dispatcher::dispatch (trait method FQN)
/// with dispatch="dynamic".
pub fn dynamic_dispatch_call(dispatcher: &dyn Dispatcher) -> String {
    dispatcher.dispatch("world")
}

/// Calls trait methods through a generic bound (trait dispatch).
/// The resolver should emit a CALLS edge to Dispatcher::dispatch (trait method FQN)
/// with dispatch="trait".
pub fn generic_dispatch_call<D: Dispatcher>(dispatcher: &D) -> String {
    dispatcher.dispatch("generic")
}

/// A scheduler that stores a boxed dynamic dispatcher.
pub struct Scheduler {
    dispatcher: Box<dyn Dispatcher>,
}

impl Scheduler {
    pub fn new(dispatcher: Box<dyn Dispatcher>) -> Self {
        Self { dispatcher }
    }

    /// Calls dispatch through the boxed trait object.
    pub fn schedule(&self, item: &str) -> String {
        self.dispatcher.dispatch(item)
    }

    /// Checks readiness through the boxed trait object.
    pub fn check_ready(&self) -> bool {
        self.dispatcher.is_ready()
    }
}

/// A function that demonstrates multiple dispatch patterns in one function body.
pub fn mixed_dispatch_demo() -> String {
    // Static dispatch on concrete FifoDispatcher
    let fifo = FifoDispatcher { capacity: 5 };
    let r1 = fifo.dispatch("fifo-item");
    
    // Static dispatch on concrete PriorityDispatcher
    let priority = PriorityDispatcher { max_priority: 3 };
    let r2 = priority.dispatch("priority-item");
    
    // Dynamic dispatch through trait object
    let dyn_ref: &dyn Dispatcher = &fifo;
    let r3 = dyn_ref.dispatch("dynamic-item");
    
    format!("{} | {} | {}", r1, r2, r3)
}
