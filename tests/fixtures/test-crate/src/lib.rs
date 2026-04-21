//! # Test Fixture Crate
//!
//! This crate provides a comprehensive test fixture for rust-brain integration tests.
//! It contains various Rust constructs to exercise the ingestion pipeline.
//!
//! ## Features
//!
//! - Public and private functions
//! - Async and unsafe functions
//! - Generic functions with trait bounds
//! - Structs with derive macros
//! - Enums with variants
//! - Trait definitions and implementations
//! - Nested modules
//! - Type aliases
//! - Constants and statics
//! - Macro usage
//! - Cross-module calls
//! - Doc comments

use serde::{Deserialize, Serialize};
use std::future::Future;
use std::pin::Pin;

// =============================================================================
// TYPE ALIASES
// =============================================================================

/// A type alias for a boxed future.
pub type BoxedFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// A simple type alias for a string result.
pub type StringResult = Result<String, String>;

/// A type alias for a callback function.
pub type Callback = fn(i32) -> i32;

// =============================================================================
// CONSTANTS AND STATICS
// =============================================================================

/// A simple constant.
pub const MAX_ITEMS: usize = 100;

/// A constant string.
pub const DEFAULT_NAME: &str = "test-fixture";

/// A mutable static (for testing purposes).
pub static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// A static string.
pub static VERSION: &str = env!("CARGO_PKG_VERSION");

// =============================================================================
// DERIVE MACROS - STRUCTS
// =============================================================================

/// A simple struct with derive macros.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct User {
    /// The user's unique identifier.
    pub id: u64,
    /// The user's name.
    pub name: String,
    /// The user's email address.
    pub email: String,
    /// Whether the user is active.
    pub active: bool,
}

impl User {
    /// Creates a new user with the given ID and name.
    ///
    /// # Arguments
    ///
    /// * `id` - The unique identifier for the user.
    /// * `name` - The user's display name.
    ///
    /// # Returns
    ///
    /// A new `User` instance.
    ///
    /// # Example
    ///
    /// ```
    /// let user = test_fixture::User::new(1, "Alice".to_string());
    /// assert_eq!(user.name, "Alice");
    /// ```
    pub fn new(id: u64, name: String) -> Self {
        Self {
            id,
            email: format!("{}@example.com", name.to_lowercase()),
            name,
            active: true,
        }
    }

    /// Deactivates the user.
    pub fn deactivate(&mut self) {
        self.active = false;
    }
}

/// A struct with generic type parameters.
#[derive(Debug, Clone, PartialEq)]
pub struct Container<T> {
    /// The contained value.
    pub value: T,
    /// A label for the container.
    pub label: String,
}

impl<T> Container<T> {
    /// Creates a new container with the given value and label.
    pub fn new(value: T, label: String) -> Self {
        Self { value, label }
    }

    /// Maps the container's value to a new value using a function.
    pub fn map<U, F>(self, f: F) -> Container<U>
    where
        F: FnOnce(T) -> U,
    {
        Container {
            value: f(self.value),
            label: self.label,
        }
    }
}

/// A tuple struct.
#[derive(Debug, Clone)]
pub struct Point(pub f64, pub f64, pub f64);

impl Point {
    /// Creates a new point at the origin.
    pub fn origin() -> Self {
        Point(0.0, 0.0, 0.0)
    }

    /// Calculates the Euclidean distance to another point.
    pub fn distance_to(&self, other: &Point) -> f64 {
        let dx = self.0 - other.0;
        let dy = self.1 - other.1;
        let dz = self.2 - other.2;
        (dx * dx + dy * dy + dz * dz).sqrt()
    }
}

/// A unit struct.
#[derive(Debug, Clone, Copy, Default)]
pub struct UnitMarker;

// =============================================================================
// ENUMS
// =============================================================================

/// An enum representing different result states.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    /// Operation completed successfully.
    Success,
    /// Operation failed with an error message.
    Error(String),
    /// Operation is pending.
    Pending,
    /// Operation was cancelled.
    Cancelled {
        /// Reason for cancellation.
        reason: String,
        /// Timestamp of cancellation.
        at: u64,
    },
}

impl Status {
    /// Checks if the status represents success.
    pub fn is_success(&self) -> bool {
        matches!(self, Status::Success)
    }

    /// Gets the error message if this is an error status.
    pub fn error_message(&self) -> Option<&str> {
        match self {
            Status::Error(msg) => Some(msg),
            _ => None,
        }
    }
}

/// An enum with explicit discriminants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ErrorCode {
    /// No error.
    None = 0,
    /// Invalid input.
    InvalidInput = 1,
    /// Not found.
    NotFound = 2,
    /// Permission denied.
    PermissionDenied = 3,
    /// Internal error.
    InternalError = 255,
}

/// A C-style enum for FFI compatibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub enum FfiResult {
    Ok,
    Err,
}

// =============================================================================
// TRAIT DEFINITIONS
// =============================================================================

pub mod dispatch_resolution;

/// A trait for processing items.
pub trait Processor {
    /// The input type for the processor.
    type Input;
    /// The output type for the processor.
    type Output;
    /// The error type for the processor.
    type Error;

    /// Processes the input and returns the output.
    fn process(&self, input: Self::Input) -> Result<Self::Output, Self::Error>;
}

/// A trait for async operations.
#[async_trait::async_trait]
pub trait AsyncHandler: Send + Sync {
    /// Handles a request asynchronously.
    async fn handle(&self, request: Request) -> Response;

    /// Returns the handler's name.
    fn name(&self) -> &str;
}

/// A trait with associated constants and types.
pub trait Configurable {
    /// The configuration type.
    type Config;

    /// The default configuration.
    const DEFAULT_CONFIG: Self::Config;

    /// Configures the instance.
    fn configure(&mut self, config: Self::Config);
}

/// A trait for cloneable handlers.
pub trait CloneHandler: AsyncHandler {
    /// Clones the handler into a boxed trait object.
    fn clone_box(&self) -> Box<dyn CloneHandler>;
}

impl<T: AsyncHandler + Clone + 'static> CloneHandler for T {
    fn clone_box(&self) -> Box<dyn CloneHandler> {
        Box::new(self.clone())
    }
}

// =============================================================================
// TRAIT IMPLEMENTATIONS
// =============================================================================

/// A simple request type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    /// The request ID.
    pub id: u64,
    /// The request payload.
    pub payload: String,
}

/// A simple response type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    /// The response status.
    pub status: Status,
    /// The response data.
    pub data: Option<String>,
}

/// A default handler implementation.
#[derive(Debug, Clone)]
pub struct DefaultHandler {
    name: String,
}

impl DefaultHandler {
    /// Creates a new default handler.
    pub fn new(name: String) -> Self {
        Self { name }
    }
}

#[async_trait::async_trait]
impl AsyncHandler for DefaultHandler {
    async fn handle(&self, request: Request) -> Response {
        Response {
            status: Status::Success,
            data: Some(format!("Handled: {}", request.payload)),
        }
    }

    fn name(&self) -> &str {
        &self.name
    }
}

/// A processor that transforms strings.
#[derive(Debug, Clone, Default)]
pub struct StringProcessor;

impl Processor for StringProcessor {
    type Input = String;
    type Output = String;
    type Error = String;

    fn process(&self, input: Self::Input) -> Result<Self::Output, Self::Error> {
        if input.is_empty() {
            Err("Input cannot be empty".to_string())
        } else {
            Ok(input.to_uppercase())
        }
    }
}

// =============================================================================
// FUNCTIONS - PUBLIC, PRIVATE, ASYNC, UNSAFE
// =============================================================================

/// A simple public function.
pub fn add(a: i32, b: i32) -> i32 {
    a + b
}

/// A public function with multiple parameters.
pub fn calculate_average(numbers: &[f64]) -> f64 {
    if numbers.is_empty() {
        return 0.0;
    }
    numbers.iter().sum::<f64>() / numbers.len() as f64
}

/// A private helper function.
fn internal_helper(x: i32) -> i32 {
    x * 2 + 1
}

/// Another private function that calls another private function.
fn private_chain(a: i32, b: i32) -> i32 {
    internal_helper(a) + internal_helper(b)
}

/// An async function.
pub async fn async_operation(input: String) -> StringResult {
    // Simulate async work
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    Ok(format!("Processed: {}", input))
}

/// An async function that returns a boxed future.
pub fn boxed_async_operation(input: String) -> BoxedFuture<StringResult> {
    Box::pin(async move {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        Ok(format!("Boxed processed: {}", input))
    })
}

/// An unsafe function.
///
/// # Safety
///
/// This function is unsafe because it performs a raw pointer dereference.
pub unsafe fn unsafe_deref(ptr: *const i32) -> i32 {
    *ptr
}

/// An unsafe function that modifies memory.
///
/// # Safety
///
/// The caller must ensure `ptr` points to valid, properly aligned memory.
pub unsafe fn unsafe_write(ptr: *mut i32, value: i32) {
    *ptr = value;
}

/// A function that calls unsafe code internally.
pub fn safe_wrapper(ptr: &mut i32, value: i32) {
    unsafe {
        unsafe_write(ptr, value);
    }
}

// =============================================================================
// GENERIC FUNCTIONS WITH TRAIT BOUNDS
// =============================================================================

/// A generic function with a simple trait bound.
pub fn identity<T>(value: T) -> T {
    value
}

/// A generic function with multiple trait bounds.
pub fn format_debug<T: std::fmt::Debug + Clone>(value: &T) -> String {
    format!("{:?}", value.clone())
}

/// A generic function with a where clause.
pub fn process_with<P>(processor: &P, input: P::Input) -> Result<P::Output, P::Error>
where
    P: Processor,
{
    processor.process(input)
}

/// A generic function with complex bounds.
pub fn compare<T>(a: &T, b: &T) -> std::cmp::Ordering
where
    T: Ord + ?Sized,
{
    a.cmp(b)
}

/// A generic function with lifetime parameters.
pub fn longest<'a, T>(a: &'a T, b: &'a T) -> &'a T
where
    T: PartialOrd,
{
    if a >= b {
        a
    } else {
        b
    }
}

/// A generic function with const generics.
pub fn create_array<T, const N: usize>(value: T) -> [T; N]
where
    T: Copy,
{
    [value; N]
}

// =============================================================================
// MACRO USAGE
// =============================================================================

/// A macro-generated struct using serde.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MacroGenerated {
    #[serde(rename = "idNumber")]
    pub id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub optional: Option<String>,
}

/// Uses a macro internally.
pub fn use_vec_macro() -> Vec<i32> {
    vec![1, 2, 3, 4, 5]
}

/// Uses the println macro.
pub fn debug_print<T: std::fmt::Debug>(value: &T) {
    println!("Debug: {:?}", value);
}

// =============================================================================
// NESTED MODULES
// =============================================================================

/// A nested module for advanced operations.
pub mod advanced {
    //! Advanced operations module.
    
    use super::*;

    /// A public function in a nested module.
    pub fn advanced_add(a: i32, b: i32) -> i32 {
        super::add(a, b) * 2
    }

    /// A private function in a nested module.
    fn private_advanced() -> i32 {
        42
    }

    /// A doubly nested module.
    pub mod inner {
        //! Inner module.

        use super::super::*;

        /// A function in a doubly nested module.
        pub fn inner_multiply(a: i32, b: i32) -> i32 {
            a * b
        }

        /// Calls across module boundaries.
        pub fn cross_module_call() -> i32 {
            super::advanced_add(5, 10) + super::super::add(3, 4)
        }

        /// A deeply nested struct.
        #[derive(Debug, Clone)]
        pub struct DeepStruct {
            pub value: i32,
        }

        impl DeepStruct {
            /// Creates a new DeepStruct.
            pub fn new(value: i32) -> Self {
                Self { value }
            }
        }
    }

    /// Re-export from inner module.
    pub use inner::DeepStruct;
}

/// A module for error handling types.
pub mod errors {
    //! Error handling types.

    use std::fmt;

    /// A custom error type.
    #[derive(Debug, Clone)]
    pub struct FixtureError {
        /// The error message.
        pub message: String,
        /// The error code.
        pub code: super::ErrorCode,
    }

    impl fmt::Display for FixtureError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "[{:?}] {}", self.code, self.message)
        }
    }

    impl std::error::Error for FixtureError {}

    impl FixtureError {
        /// Creates a new error.
        pub fn new(message: impl Into<String>, code: super::ErrorCode) -> Self {
            Self {
                message: message.into(),
                code,
            }
        }
    }
}

// =============================================================================
// CROSS-MODULE CALLS
// =============================================================================

/// Calls functions from different modules.
pub fn cross_module_demo() -> i32 {
    // Call within same module
    let a = add(1, 2);
    
    // Call from nested module
    let b = advanced::advanced_add(3, 4);
    
    // Call from doubly nested module
    let c = advanced::inner::inner_multiply(5, 6);
    
    // Use types from other modules
    let _deep = advanced::DeepStruct::new(10);
    
    a + b + c
}

/// Uses error types from the errors module.
pub fn may_fail(input: &str) -> Result<String, errors::FixtureError> {
    if input.is_empty() {
        Err(errors::FixtureError::new(
            "Input cannot be empty",
            ErrorCode::InvalidInput,
        ))
    } else {
        Ok(input.to_uppercase())
    }
}

// =============================================================================
// IMPLEMENTATIONS FOR STANDARD TRAITS
// =============================================================================

impl std::fmt::Display for User {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "User({}, {})", self.id, self.name)
    }
}

impl Default for User {
    fn default() -> Self {
        Self {
            id: 0,
            name: String::new(),
            email: String::new(),
            active: true,
        }
    }
}

impl From<(u64, String)> for User {
    fn from((id, name): (u64, String)) -> Self {
        Self::new(id, name)
    }
}

impl<T: std::fmt::Debug> std::fmt::Display for Container<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Container({}, {:?})", self.label, self.value)
    }
}

// =============================================================================
// CLOSURE AND HIGHER-ORDER FUNCTIONS
// =============================================================================

/// Takes a closure as an argument.
pub fn apply_function<F>(value: i32, f: F) -> i32
where
    F: FnOnce(i32) -> i32,
{
    f(value)
}

/// Returns a closure.
pub fn create_multiplier(factor: i32) -> impl Fn(i32) -> i32 {
    move |x| x * factor
}

/// A higher-order function.
pub fn compose<A, B, C, F, G>(f: F, g: G) -> impl Fn(A) -> C
where
    F: Fn(B) -> C,
    G: Fn(A) -> B,
{
    move |a| f(g(a))
}

// =============================================================================
// RE-EXPORTS
// =============================================================================

pub use errors::FixtureError;
pub use advanced::DeepStruct;

// =============================================================================
// UNIT TESTS
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add() {
        assert_eq!(add(2, 3), 5);
    }

    #[test]
    fn test_user_creation() {
        let user = User::new(1, "Alice".to_string());
        assert_eq!(user.id, 1);
        assert_eq!(user.name, "Alice");
        assert!(user.active);
    }

    #[test]
    fn test_container_map() {
        let container = Container::new(5, "test".to_string());
        let mapped = container.map(|x| x * 2);
        assert_eq!(mapped.value, 10);
    }

    #[test]
    fn test_status() {
        let success = Status::Success;
        assert!(success.is_success());

        let error = Status::Error("something went wrong".to_string());
        assert_eq!(error.error_message(), Some("something went wrong"));
    }

    #[test]
    fn test_string_processor() {
        let processor = StringProcessor;
        let result = processor.process("hello".to_string());
        assert_eq!(result, Ok("HELLO".to_string()));
    }

    #[test]
    fn test_cross_module() {
        let result = cross_module_demo();
        assert_eq!(result, 3 + 14 + 30); // 3 + 14 + 30 = 47
    }

    #[tokio::test]
    async fn test_async_operation() {
        let result = async_operation("test".to_string()).await;
        assert_eq!(result, Ok("Processed: test".to_string()));
    }
}
