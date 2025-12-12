# Contributing to ForgeLSP

## Getting Started

### Prerequisites
- Rust 1.75+ (nightly with edition 2024 support)
- Git
- IDE with Rust support (VS Code, RustRover, etc.)

### Initial Setup
```bash
# Clone repository
git clone https://github.com/tryforge/forgelsp
cd forgelsp

# Build project
cargo build

# Run tests
cargo test

# Run LSP server
cargo run
```

## Development Workflow

### 1. Create a Branch
```bash
git checkout -b feature/your-feature-name
```

### 2. Make Changes
Follow the guidelines in:
- `.guidelines/CODING_STYLE.md`
- `.guidelines/ARCHITECTURE.md`
- `.guidelines/DOCUMENTATION.md`

### 3. Test Your Changes
```bash
# Format code
cargo fmt

# Run clippy
cargo clippy --all-targets -- -D warnings

# Run tests
cargo test

# Build release
cargo build --release
```

### 4. Commit Changes
```bash
git add .
git commit -m "feat: add new LSP feature"
```

**Commit Message Format:**
```
<type>: <description>

[optional body]

[optional footer]
```

**Types:**
- `feat`: New feature
- `fix`: Bug fix
- `docs`: Documentation changes
- `style`: Code style changes (formatting, etc.)
- `refactor`: Code refactoring
- `test`: Adding tests
- `chore`: Maintenance tasks

### 5. Push and Create PR
```bash
git push origin feature/your-feature-name
```

Create a Pull Request on GitHub with:
- Clear description of changes
- Reference to related issues
- Test results

## Code Review Process

### Requirements
- [ ] All tests pass
- [ ] Code formatted with `cargo fmt`
- [ ] No clippy warnings
- [ ] Documentation updated
- [ ] Examples provided (if applicable)

### Review Checklist
Reviewers will check:
- Code quality and readability
- Test coverage
- Documentation completeness
- Performance implications
- Breaking changes

## Adding New Features

### LSP Features
1. Add handler in `src/server.rs`
2. Register capability in `initialize()`
3. Implement core logic in dedicated module
4. Add tests
5. Update documentation

### Parser Enhancements
1. Modify `src/parser.rs`
2. Add test cases for new syntax
3. Update `docs/parser.md`
4. Ensure backward compatibility

### Metadata Extensions
1. Update `src/metadata.rs`
2. Ensure cache compatibility
3. Add validation for new fields
4. Update documentation

## Testing Guidelines

### Unit Tests
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_function_name() {
        let input = "test data";
        let expected = "expected result";
        assert_eq!(my_function(input), expected);
    }
}
```

### Integration Tests
Place in `tests/` directory:
```rust
// tests/integration_test.rs
use forgevsc::*;

#[test]
fn test_full_workflow() {
    // Test complete feature flow
}
```

### Test Coverage
Aim for:
- Critical paths: 100%
- Public APIs: 90%+
- Overall: 70%+

## Documentation Requirements

### Code Documentation
- All public items must have doc comments
- Module-level documentation required
- Examples for complex functions

### User Documentation
- Update `README.md` for visible changes
- Add to `docs/` for new modules
- Update examples if syntax changes

## Performance Considerations

### Benchmarking
```rust
#[bench]
fn bench_parser(b: &mut Bencher) {
    let code = "...";
    b.iter(|| {
        parser.parse(code)
    });
}
```

### Profiling
```bash
cargo flamegraph --bin forgevsc
```

### Memory Usage
Monitor with:
```bash
/usr/bin/time -v cargo run
```

## Debugging

### Logging
Add logging for debugging:
```rust
spawn_log(
    client,
    MessageType::LOG,
    format!("[DEBUG] Processing: {item}")
);
```

### LSP Trace
Enable in VS Code:
```json
{
    "forgescript.trace.server": "verbose"
}
```

## Release Process

### Version Bumping
Update `Cargo.toml`:
```toml
[package]
version = "0.2.0"  # Semantic versioning
```

### Changelog
Update `CHANGELOG.md`:
```markdown
## [0.2.0] - 2024-12-XX
### Added
- New features
### Changed
- Modifications
### Fixed
- Bug fixes
```

### Git Tag
```bash
git tag -a v0.2.0 -m "Release version 0.2.0"
git push origin v0.2.0
```

## Common Patterns

### Adding a New LSP Handler
```rust
// 1. In server.rs
async fn new_feature(&self, params: NewParams) -> Result<NewResponse> {
    // Implementation
}

// 2. Register in initialize()
new_feature_provider: Some(true)

// 3. Add tests
#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn test_new_feature() {
        // Test implementation
    }
}
```

### Adding Configuration
```rust
// 1. Update ForgeConfig in utils.rs
pub struct ForgeConfig {
    pub new_option: Option<bool>,
    // ...
}

// 2. Handle in server initialization
if let Some(value) = config.new_option {
    // Apply setting
}
```

## Getting Help

### Resources
- [Rust Book](https://doc.rust-lang.org/book/)
- [LSP Specification](https://microsoft.github.io/language-server-protocol/)
- [Tower LSP Docs](https://docs.rs/tower-lsp)
- Project documentation in `docs/`

### Communication
- GitHub Issues for bugs
- GitHub Discussions for questions
- Pull Requests for contributions

## Code of Conduct

- Be respectful and constructive
- Welcome newcomers
- Focus on code quality
- Provide helpful feedback
- Acknowledge contributions

Thank you for contributing to ForgeLSP!
