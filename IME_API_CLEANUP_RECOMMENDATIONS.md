# IME API Cleanup Recommendations

## Executive Summary

Based on analysis of cosmic-comp (the primary smithay-based compositor with IME support), we have identified several public APIs in smithay's input_method module that are unused. This document provides recommendations for API cleanup to improve maintainability and prevent misuse.

## Analysis Results

### InputMethodHandle APIs

| Function | Status | Used By | Recommendation |
|----------|--------|---------|----------------|
| `set_active_instance()` | ✅ Used | cosmic-comp | **Keep public** |
| `clear_active_instance()` | ✅ Used | cosmic-comp | **Keep public** |
| `keyboard_grabbed()` | ✅ Used | cosmic-comp | **Keep public** |
| `send_keymap_to_grab()` | ✅ Used | cosmic-comp | **Keep public** |
| `activate_input_method()` | ✅ Used | cosmic-comp | **Keep public** |
| `deactivate_input_method()` | ✅ Used | cosmic-comp | **Keep public** |
| `list_instances()` | ❌ Unused | None | **Consider removing** |

**Usage: 6/7 functions (85.7%)**

### PopupSurface APIs

| Function | Status | Used By | Recommendation |
|----------|--------|---------|----------------|
| `get_parent()` | ✅ Used | cosmic-comp | **Keep public** |
| `alive()` | ❌ Unused | None | **Keep public** (reasonable API) |
| `wl_surface()` | ❌ Unused | None | **Keep public** (escape hatch) |
| `set_parent()` | ❌ Unused | None | **Make internal** |
| `location()` | ❌ Unused | None | **Keep public** (reasonable API) |
| `set_location()` | ❌ Unused | None | **Make internal** |
| `text_input_rectangle()` | ❌ Unused | None | **Keep public** (reasonable API) |
| `set_text_input_rectangle()` | ❌ Unused | None | **Make internal** |

**Usage: 1/8 functions (12.5%)**

### InputMethodManagerState APIs

| Function | Status | Used By | Recommendation |
|----------|--------|---------|----------------|
| `new()` | ✅ Used | All compositors | **Keep public** |
| `global()` | ❌ Unused | None | **Keep public** (may be needed) |

**Usage: 1/2 functions (50%)**

## Detailed Recommendations

### Priority 1: Make Internal (Prevent Misuse)

These functions should be made `pub(crate)` to prevent compositor misuse while keeping them available for internal protocol handling:

#### 1. `PopupSurface::set_parent()`
```rust
// Change from:
pub fn set_parent(&mut self, parent: Option<PopupParent>)

// To:
pub(crate) fn set_parent(&mut self, parent: Option<PopupParent>)
```

**Rationale:**
- Parent is set during popup creation via protocol
- Changing parent mid-flight could break compositor assumptions
- No legitimate use case for compositor to change parent
- Only used internally during popup setup

#### 2. `PopupSurface::set_location()`
```rust
// Change from:
pub fn set_location(&self, location: Point<i32, Logical>)

// To:
pub(crate) fn set_location(&self, location: Point<i32, Logical>)
```

**Rationale:**
- Location should be derived from `text_input_rectangle`
- Manual setting could conflict with protocol semantics
- Compositor should query location, not set it
- Only used internally by protocol handler

#### 3. `PopupSurface::set_text_input_rectangle()`
```rust
// Change from:
pub fn set_text_input_rectangle(&mut self, x: i32, y: i32, width: i32, height: i32)

// To:
pub(crate) fn set_text_input_rectangle(&mut self, x: i32, y: i32, width: i32, height: i32)
```

**Rationale:**
- This is set by text input protocol events, not compositor
- Compositor manually setting this would violate protocol
- Only used internally to receive protocol updates
- Getter should remain public for compositor to query

**Impact:** Low risk - These are already unused, making them internal prevents future misuse.

### Priority 2: Consider Removing

#### `InputMethodHandle::list_instances()`

**Current Status:**
- ❌ Not used in cosmic-comp
- ❌ Not used in any other project in the codebase
- Defined in: `smithay/src/wayland/input_method/input_method_handle.rs:144-154`

**Options:**

**Option A: Remove entirely**
```rust
// Delete:
pub fn list_instances(&self) -> Vec<(String, u32, bool)> { ... }
```
- Simplest approach
- Reduces API surface
- Can always add back if needed

**Option B: Keep with documentation**
```rust
/// List all registered input method instances.
/// 
/// Returns a vector of tuples: (app_id, serial, is_active)
/// 
/// # Use Cases
/// - Debugging tools
/// - IME picker UI
/// - Compositor configuration interfaces
///
/// Most compositors don't need this as they activate IMEs by app_id directly.
pub fn list_instances(&self) -> Vec<(String, u32, bool)> { ... }
```
- Documents intended use cases
- Keeps API for potential future use
- Low maintenance burden (simple function)

**Recommendation:** Keep with enhanced documentation (Option B)

This provides value for debugging and potential IME picker UIs while documenting that it's not required for basic IME functionality.

### Priority 3: Keep As-Is

These functions are currently unused but provide legitimate compositor APIs:

#### `PopupSurface::alive()`
- Valid use case: Check if surface still exists before operations
- Similar pattern used for other surface types in cosmic-comp
- Low cost to maintain

#### `PopupSurface::wl_surface()`
- Provides escape hatch for advanced surface manipulation
- Essential for compositors with custom rendering needs
- Standard pattern across smithay surface types

#### `PopupSurface::location()` and `text_input_rectangle()`
- Enable custom popup positioning logic
- Read-only queries are safe
- May be needed by compositors with advanced IME popup handling

#### `InputMethodManagerState::global()`
- Standard pattern for all protocol managers
- May be needed for dynamic global management
- Zero cost abstraction

## Implementation Plan

### Phase 1: Low-Risk Changes (Recommended for next release)

1. Make `PopupSurface::set_parent()` internal
2. Make `PopupSurface::set_location()` internal
3. Make `PopupSurface::set_text_input_rectangle()` internal
4. Add documentation to `list_instances()` explaining use cases

**Files to modify:**
- `smithay/src/wayland/input_method/input_method_popup_surface.rs`
- `smithay/src/wayland/input_method/input_method_handle.rs`

**Breaking changes:** None (unused APIs becoming internal)

### Phase 2: Evaluation (Before removing anything)

1. Survey other smithay-based compositors
2. Check if any external projects use these APIs
3. Document findings
4. Decide on `list_instances()` removal

### Phase 3: Documentation Improvements

Add examples and clarify API intentions:

```rust
/// # Example
/// ```no_run
/// // Compositors typically don't need to list instances
/// // They activate IMEs by app_id based on keyboard layout:
/// if input_method_handle.set_active_instance("fcitx5") {
///     input_method_handle.activate_input_method(state, surface);
/// }
/// 
/// // Listing is useful for debug tools:
/// for (app_id, serial, is_active) in input_method_handle.list_instances() {
///     println!("IME: {} (serial: {}, active: {})", app_id, serial, is_active);
/// }
/// ```
```

## Testing Strategy

1. **Build test:** Ensure cosmic-comp compiles after changes
2. **Runtime test:** Verify IME functionality unchanged in cosmic-comp
3. **API test:** Confirm internal functions not accessible from cosmic-comp
4. **Documentation test:** Build rustdoc and verify examples

## Compatibility

### Semantic Versioning

- Making public APIs internal: **Breaking change** (minor version bump)
- Removing public APIs: **Breaking change** (minor version bump)
- Documentation improvements: **Non-breaking** (patch version)

### Migration Guide

For any compositor using the functions being made internal:

```rust
// Before:
popup.set_parent(Some(parent));
popup.set_location(Point::from((x, y)));

// After:
// These are now handled automatically by the protocol.
// If you need custom behavior, file an issue explaining your use case.
```

## Benefits

1. **Clearer API surface:** Compositors know which functions they should use
2. **Prevent misuse:** Internal functions can't be misused by compositors
3. **Better documentation:** Clear intent for each public function
4. **Easier maintenance:** Fewer public APIs to maintain compatibility for
5. **Type safety:** Compiler prevents incorrect API usage

## Risks

**Low risk:**
- Functions being made internal are already unused
- Can be reverted if legitimate use case emerges

**Mitigation:**
- Document the changes in CHANGELOG
- Provide migration guide
- Survey other compositors before finalizing

## Conclusion

The IME API in smithay is well-designed with high usage of essential functions. The main opportunity for improvement is making setter functions internal to prevent misuse while keeping getters public for flexibility. The `list_instances()` function should be kept but better documented to explain its debugging/UI use cases.

**Recommended immediate action:** Implement Phase 1 changes to prevent future API misuse.

---

**References:**
- Analysis: `smithay/UNUSED_IME_API.md`
- Bug fix: `cosmic-comp/KEYMAP_BUG_FIX.md`
- Compositor usage: `cosmic-comp/src/wayland/handlers/input_method.rs`
