# Documentation Standards

## Module Documentation

### Every Module Must Have
```rust
//! # Module Name
//!
//! Brief one-line description of module purpose.
//!
//! ## Detailed Description
//!
//! Comprehensive explanation including:
//! - Primary responsibilities
//! - Key algorithms or patterns used
//! - Integration with other modules
//! - Important constraints or considerations
//!
//! ## Example Usage (if applicable)
//!
//! ```rust
//! use crate::module::Function;
//! 
//! let result = Function::new();
//! ```
```

## Function Documentation

### Public Functions
```rust
/// Brief one-line description of what the function does.
///
/// More detailed explanation if needed. Explain the purpose,
/// behavior, and any important edge cases.
///
/// # Arguments
///
/// * `param1` - Description of first parameter
/// * `param2` - Description of second parameter
///
/// # Returns
///
/// Description of return value and what it represents
///
/// # Errors
///
/// When and why this function might return an error (if Result)
///
/// # Examples
///
/// ```rust
/// let result = my_function(arg1, arg2);
/// assert_eq!(result, expected);
/// ```
pub fn my_function(param1: Type1, param2: Type2) -> ReturnType {
    // Implementation
}
```

### Private Functions
```rust
/// Brief description (one line is often sufficient)
fn helper_function(param: Type) -> Result {
    // Implementation
}
```

## Struct & Enum Documentation

### Structs
```rust
/// Brief description of the struct's purpose.
///
/// Detailed explanation of:
/// - What this struct represents
/// - When to use it
/// - Important invariants or constraints
pub struct MyStruct {
    /// Description of this field
    pub field1: Type1,
    
    /// Description of this field
    /// Can span multiple lines if needed
    field2: Type2,
}
```

### Enums
```rust
/// Brief description of what this enum represents
pub enum MyEnum {
    /// Description of this variant
    Variant1,
    
    /// Description of this variant
    Variant2 { field: Type },
}
```

## Inline Comments

### When to Comment

**✅ DO comment:**
- Complex algorithms
- Non-obvious design decisions
- Why certain approaches were taken
- Workarounds or limitations
- Performance-critical sections

**❌ DON'T comment:**
- Obvious code that speaks for itself
- What the code does (the code shows this)
- Redundant statements

### Examples

**✅ Good:**
```rust
// Use Trie for O(k) lookup instead of HashMap O(1) because we need
// prefix matching for partial function names during autocompletion
let trie = FunctionTrie::new();

// Handle Rust 2024 let chains which require special escaping
if let Some(config) = load_config()
    && let Some(funcs) = config.custom_functions {
    // ...
}
```

**❌ Bad:**
```rust
// Create new trie
let trie = FunctionTrie::new();

// Loop through items
for item in items {
    // Process item
    process(item);
}
```

## Documentation in `docs/` Directory

### Module Documentation Files

Each source file should have a corresponding markdown file:
- `src/parser.rs` → `docs/parser.md`
- `src/metadata.rs` → `docs/metadata.md`

### Structure
```markdown
# module_name.rs

## Overview
High-level description of module purpose

## Core Data Structures
Description of main types with code examples

## Key Functions
Explanation of important public APIs

## Algorithms
Detailed explanation of non-trivial algorithms

## Integration Points
How this module interacts with others

## Examples
Common usage patterns
```

### Code Examples in Markdown
```markdown
```rust
// Example code with syntax highlighting
pub fn example() {
    println!("Hello");
}
```
```

## README.md

### Required Sections
1. **Project Description**
2. **Features**
3. **Project Structure**
4. **Installation**
5. **Usage**
6. **Documentation Links**
7. **Dependencies**
8. **Building**
9. **Contributing**
10. **License**

### Keep Updated
- Update when adding new features
- Update when changing structure
- Update dependency table when modifying `Cargo.toml`

## Cargo.toml Documentation

### Package Metadata
```toml
[package]
name = "forgevsc"
version = "0.1.0"
edition = "2024"
description = "Detailed description for crates.io"
license = "MIT"
repository = "https://github.com/owner/repo"
keywords = ["keyword1", "keyword2"]  # Max 5
categories = ["category1", "category2"]  # From crates.io list
authors = ["Team Name"]
```

## URL Formatting

### In Rust Doc Comments
```rust
/// See <https://example.com/docs> for more information
/// 
/// Repository: <https://github.com/owner/repo>
```

### In Markdown
```markdown
See [documentation](https://example.com/docs) for details.

For raw URLs: <https://example.com>
```

## Examples Section

### Practical Examples
Every module should include practical examples in its documentation:

```rust
/// # Examples
///
/// Basic usage:
/// ```
/// let parser = ForgeScriptParser::new(manager, code);
/// let result = parser.parse();
/// ```
///
/// With error handling:
/// ```
/// match parser.parse() {
///     Ok(result) => println!("Parsed {} functions", result.functions.len()),
///     Err(e) => eprintln!("Parse error: {}", e),
/// }
/// ```
```

## Documentation Testing

### Ensure Examples Compile
```bash
cargo test --doc
```

### Run Documentation Tests
All `///` examples with code blocks are tested by default.

## Changelog

### Keep `CHANGELOG.md` Updated
```markdown
# Changelog

## [Unreleased]
### Added
- New feature X
### Changed
- Modified behavior Y
### Fixed
- Bug Z

## [0.1.0] - 2024-12-12
### Added
- Initial release
```

## Documentation Review Checklist

Before committing, ensure:
- [ ] All public items have doc comments
- [ ] Module-level documentation exists
- [ ] Examples compile and run
- [ ] External links work
- [ ] Code examples use proper syntax highlighting
- [ ] Complex algorithms explained
- [ ] Integration points documented
- [ ] README is up-to-date
