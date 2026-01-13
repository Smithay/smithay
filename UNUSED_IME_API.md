# Unused IME API Analysis in Smithay

Analysis of public functions in smithay's input_method module to identify unused APIs based on cosmic-comp usage patterns.

## Summary

Based on cosmic-comp (the reference compositor), the following public functions in smithay's IME implementation are **UNUSED**:

### InputMethodHandle

1. ✅ **USED**: `set_active_instance(&self, app_id: &str) -> bool`
   - Used in: `cosmic-comp/src/wayland/handlers/input_method.rs:212`
   - Purpose: Set which IME is active based on keyboard layout

2. ✅ **USED**: `clear_active_instance<D>(&self, state: &mut D)`
   - Used in: `cosmic-comp/src/wayland/handlers/input_method.rs:252`
   - Purpose: Deactivate IME when switching to unmapped layout

3. ✅ **USED**: `keyboard_grabbed(&self) -> bool`
   - Used in: `cosmic-comp/src/config/mod.rs:832`
   - Purpose: Check if IME has keyboard grab before sending keymap

4. ✅ **USED**: `send_keymap_to_grab<D>(&self, keyboard_handle: &KeyboardHandle<D>)`
   - Used in: `cosmic-comp/src/config/mod.rs:836`
   - Purpose: Send keymap updates to IME when layout changes

5. ✅ **USED**: `activate_input_method<D>(&self, state: &mut D, surface: &WlSurface)`
   - Used in: `cosmic-comp/src/wayland/handlers/input_method.rs:224`
   - Purpose: Activate IME on focused text input

6. ✅ **USED**: `deactivate_input_method<D>(&self, state: &mut D)`
   - Used in: `cosmic-comp/src/wayland/handlers/input_method.rs:159`
   - Purpose: Deactivate IME when no mapping exists

7. ❌ **UNUSED**: `list_instances(&self) -> Vec<(String, u32, bool)>`
   - **Location**: `smithay/src/wayland/input_method/input_method_handle.rs:144-154`
   - **Purpose**: Get list of all registered IME instances with app_ids
   - **Why unused**: cosmic-comp only needs to activate by app_id, not enumerate all
   - **Potential use case**: UI to show available IMEs, debugging tools

### PopupSurface (InputMethod)

1. ✅ **USED**: `get_parent(&self) -> Option<&PopupParent>`
   - Used in: `cosmic-comp/src/shell/mod.rs:1732,1822`
   - Purpose: Find parent surface for IME popup positioning

2. ❌ **UNUSED**: `alive(&self) -> bool`
   - **Location**: `smithay/src/wayland/input_method/input_method_popup_surface.rs:53-57`
   - **Purpose**: Check if popup surface is still alive
   - **Why unused**: cosmic-comp doesn't need to manually check; protocol handles cleanup
   - **Note**: Similar methods for other surface types ARE used in cosmic-comp

3. ❌ **UNUSED**: `wl_surface(&self) -> &WlSurface`
   - **Location**: `smithay/src/wayland/input_method/input_method_popup_surface.rs:61-65`
   - **Purpose**: Access underlying wl_surface
   - **Why unused**: cosmic-comp treats IME popups opaquely via PopupKind
   - **Note**: Used for XDG popups extensively, but not IME popups

4. ❌ **UNUSED**: `set_parent(&mut self, parent: Option<PopupParent>)`
   - **Location**: `smithay/src/wayland/input_method/input_method_popup_surface.rs:71-75`
   - **Purpose**: Set the IME popup's parent surface
   - **Why unused**: Parent is set during creation, no need to change
   - **Risk**: Changing parent mid-flight could break compositor logic

5. ❌ **UNUSED**: `location(&self) -> Point<i32, Logical>`
   - **Location**: `smithay/src/wayland/input_method/input_method_popup_surface.rs:76-80`
   - **Purpose**: Get popup location relative to parent
   - **Why unused**: cosmic-comp uses smithay's PopupManager which handles positioning
   - **Note**: Position is managed internally by the protocol implementation

6. ❌ **UNUSED**: `set_location(&self, location: Point<i32, Logical>)`
   - **Location**: `smithay/src/wayland/input_method/input_method_popup_surface.rs:86-90`
   - **Purpose**: Set popup location
   - **Why unused**: Location is computed from text_input_rectangle
   - **Risk**: Manual location setting could conflict with protocol semantics

7. ❌ **UNUSED**: `text_input_rectangle(&self) -> Rectangle<i32, Logical>`
   - **Location**: `smithay/src/wayland/input_method/input_method_popup_surface.rs:92-96`
   - **Purpose**: Get the text input region to avoid obscuring
   - **Why unused**: cosmic-comp doesn't reposition IME popups based on this
   - **Note**: Rectangle is set via protocol, but compositor doesn't query it

8. ❌ **UNUSED**: `set_text_input_rectangle(&mut self, x: i32, y: i32, width: i32, height: i32)`
   - **Location**: `smithay/src/wayland/input_method/input_method_popup_surface.rs:101-106`
   - **Purpose**: Set text input rectangle (also updates location)
   - **Why unused**: This is set internally by the protocol handler
   - **Risk**: Compositor should not manually set this; it comes from the text input

### InputMethodManagerState

1. ✅ **USED**: `new<D, F>(display: &DisplayHandle, filter: F) -> Self`
   - Used implicitly during compositor initialization

2. ❌ **UNUSED**: `global(&self) -> GlobalId`
   - **Location**: `smithay/src/wayland/input_method/mod.rs:162-166`
   - **Purpose**: Get the GlobalId of the input method manager
   - **Why unused**: cosmic-comp never needs to reference the global after creation
   - **Potential use case**: Dynamic global management, debug tools

## Recommendations

### Definitely Keep (Used or Essential)
- ✅ All InputMethodHandle activation/deactivation methods
- ✅ `PopupSurface::get_parent()` - Essential for compositor logic

### Consider Making Internal (pub(crate))

1. **`PopupSurface::set_parent()`**
   - Risk: Compositor shouldn't change parent mid-flight
   - Current status: Only used internally during popup creation
   - **Recommendation**: Make `pub(crate)` to prevent misuse

2. **`PopupSurface::set_location()`**
   - Risk: Location should be derived from text_input_rectangle
   - Current status: Set internally, no compositor use case
   - **Recommendation**: Make `pub(crate)` or private

3. **`PopupSurface::set_text_input_rectangle()`**
   - Risk: Should only be set via protocol events, not compositor
   - Current status: Protocol-driven, no compositor override needed
   - **Recommendation**: Make `pub(crate)` or private

### Consider Removing or Marking as Debug-Only

1. **`InputMethodHandle::list_instances()`**
   - Use case: Debug tools, IME picker UI
   - Current usage: None in cosmic-comp
   - **Recommendation**: Keep but document as "for debugging/UI only"

2. **`InputMethodManagerState::global()`**
   - Use case: Dynamic global management
   - Current usage: None in cosmic-comp
   - **Recommendation**: Keep for completeness

### Keep As-Is (Valid API, Not Yet Used)

1. **`PopupSurface::alive()`**
   - Valid use case: Manual lifetime management
   - **Recommendation**: Keep public; might be needed for complex scenarios

2. **`PopupSurface::wl_surface()`**
   - Valid use case: Advanced surface manipulation
   - **Recommendation**: Keep public; provides escape hatch

3. **`PopupSurface::location()` and `text_input_rectangle()`**
   - Valid use case: Custom popup positioning logic
   - **Recommendation**: Keep public for advanced compositors

## Usage Statistics

### InputMethodHandle: 6/7 functions used (85.7%)
- Only `list_instances()` unused

### PopupSurface: 1/8 functions used (12.5%)
- Only `get_parent()` used
- Most functions provide low-level control not needed by typical compositors

### InputMethodManagerState: 1/2 functions used (50%)
- `global()` unused but reasonable to keep

## Impact Analysis

### Low Risk Changes (Recommended)
- Make `PopupSurface::set_parent()` internal
- Make `PopupSurface::set_location()` internal  
- Make `PopupSurface::set_text_input_rectangle()` internal

These prevent misuse while keeping essential functionality public.

### Medium Risk Changes
- Remove `list_instances()` if no other compositors use it
- Check if wlroots-based compositors use these APIs

### High Risk Changes (Not Recommended)
- Removing `PopupSurface::wl_surface()` - provides important escape hatch
- Removing location/rectangle getters - might be needed for custom positioning

## Conclusion

The IME API in smithay is fairly lean, with most unused functions providing low-level control that cosmic-comp doesn't need. The main opportunity for cleanup is making some `PopupSurface` setters internal (`pub(crate)`) to prevent misuse, while keeping getters public for flexibility.

The `list_instances()` function could potentially be removed or moved to a debugging trait if no other compositors use it.