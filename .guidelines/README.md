# ForgeLSP Guidelines

This directory contains comprehensive documentation for working with the ForgeLSP codebase.

## For All Contributors

### Essential Reading
1. **[CODING_STYLE.md](./CODING_STYLE.md)** - Rust style conventions, formatting, and patterns
2. **[ARCHITECTURE.md](./ARCHITECTURE.md)** - System design, module responsibilities, and data flow
3. **[DOCUMENTATION.md](./DOCUMENTATION.md)** - How to document code, APIs, and modules
4. **[CONTRIBUTING.md](./CONTRIBUTING.md)** - Development workflow, testing, and PR process

### Quick Start
```bash
# 1. Setup
git clone https://github.com/tryforge/forgelsp
cd forgelsp
cargo build

# 2. Before committing
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test

# 3. Follow guidelines
# - Read CODING_STYLE.md for Rust patterns
# - Check ARCHITECTURE.md to understand system design
# - Update docs per DOCUMENTATION.md standards
# - Follow CONTRIBUTING.md workflow
```

## For AI Assistants

### Primary Guide
**[AI_GUIDELINES.md](./AI_GUIDELINES.md)** - Comprehensive guide for AI assistants including:
- Quick reference for all modules
- Common patterns and anti-patterns
- LSP-specific code examples
- Performance tips
- Debugging assistance
- Questions to ask before committing

### Key Principles
1. **Follow Rust 2024:** Use modern idioms (let-else, inline format strings)
2. **Safety First:** Proper error handling, no unwrap() in production paths
3. **Document Everything:** Public APIs must have doc comments
4. **Test Changes:** Run full verification before proposing changes
5. **Performance Matters:** Consider threading, caching, and allocations

## Directory Structure

```
.guidelines/
├── README.md              # This file - overview and navigation
├── CODING_STYLE.md        # Rust conventions and formatting
├── ARCHITECTURE.md        # System design and module interaction
├── DOCUMENTATION.md       # Doc comment and markdown standards
├── CONTRIBUTING.md        # Development workflow and PR process
└── AI_GUIDELINES.md       # AI assistant-specific guidance
```

## Related Documentation

### In Repository Root
- **README.md** - Project overview and quick start
- **CHANGELOG.md** - Version history and changes
- **Cargo.toml** - Dependencies and package metadata

### In `docs/` Directory
Detailed module documentation:
- `docs/main.md` - Entry point initialization
- `docs/server.md` - LSP server implementation
- `docs/parser.md` - ForgeScript parsing
- `docs/metadata.md` - Function metadata management
- `docs/semantic.md` - Semantic token extraction
- `docs/hover.md` - Hover provider
- `docs/diagnostics.md` - Error reporting
- `docs/utils.md` - Utilities and configuration

## Quick Reference

### Key Technologies
- **Language:** Rust 2024 Edition
- **Framework:** Tower LSP
- **Async Runtime:** Tokio
- **Testing:** cargo test, integration tests

### Project Statistics
- **Lines of Code:** ~3,500
- **Modules:** 8 source files
- **Dependencies:** 12 crates
- **Documentation:** 8 module docs + 5 guidelines

### Code Quality Standards
- ✅ Zero clippy warnings (standard mode)
- ✅ 100% formatted with cargo fmt
- ✅ Comprehensive documentation
- ✅ Full LSP protocol compliance

## Getting Help

### For Contributors
1. Check relevant guideline document
2. Read module documentation in `docs/`
3. Review existing code for patterns
4. Open GitHub issue for questions

### For AI Assistants
1. Read AI_GUIDELINES.md thoroughly
2. Check module-specific docs in `docs/`
3. Follow patterns in CODING_STYLE.md
4. Verify against ARCHITECTURE.md

## Maintenance

### Keeping Guidelines Updated
When making significant changes:
- **New module?** → Add to ARCHITECTURE.md, create `docs/module.md`
- **New pattern?** → Update CODING_STYLE.md
- **New workflow?** → Update CONTRIBUTING.md
- **API changes?** → Update relevant `docs/*.md`

### Review Schedule
- Review guidelines quarterly
- Update for Rust edition changes
- Sync with community best practices

## Version History

- **v1.0** (2024-12-12) - Initial comprehensive guidelines
  - Complete coding style guide
  - Full architecture documentation
  - Documentation standards
  - Contributing guidelines
  - AI assistant guidelines

---

**Remember:** These guidelines exist to ensure consistency, quality, and maintainability. When in doubt, prioritize clarity and correctness over cleverness.
