use crate::{frame_budget, startup_budget, GuestState, GUEST_HEAP_LIMIT};
use pocket_drivers::{PackageBytes, PackageSource, SourceError};
use pocket_runtime::{Profile, RuntimeConfig};
use pocket_runtime_cabi::{abi::*, embedding};
use pocketjs_core::spec::BINDING_GENERATION;
use std::{
    borrow::Cow,
    ffi::{c_char, c_void, CStr, CString},
    ptr,
    time::{Duration, Instant},
};

#[repr(C)]
struct JSRuntime {
    _private: [u8; 0],
}
#[repr(C)]
struct JSContext {
    _private: [u8; 0],
}
#[repr(C)]
#[derive(Clone, Copy)]
struct JSValue {
    u: [u64; 1],
    tag: i64,
}
#[repr(C)]
struct QuickJsAdapter {
    _private: [u8; 0],
}
type Intr = unsafe extern "C" fn(*mut JSRuntime, *mut c_void) -> i32;
type Job = unsafe extern "C" fn(*mut c_void) -> i32;
#[repr(C)]
struct Vtable {
    size: usize,
    evaluate: Option<
        unsafe extern "C" fn(*mut QuickJsAdapter, *mut PocketRuntime, *const u8, usize) -> i32,
    >,
    frame: Option<unsafe extern "C" fn(*mut QuickJsAdapter, *const c_void) -> i32>,
    collect: Option<unsafe extern "C" fn(*mut QuickJsAdapter) -> i32>,
    shutdown: Option<unsafe extern "C" fn(*mut QuickJsAdapter)>,
    initialize: Option<
        unsafe extern "C" fn(
            *mut QuickJsAdapter,
            *mut PocketRuntime,
            *const PocketGuestEnvironment,
        ) -> i32,
    >,
}
extern "C" {
    fn JS_NewRuntime() -> *mut JSRuntime;
    fn JS_SetMemoryLimit(runtime: *mut JSRuntime, limit: usize);
    fn JS_SetInterruptHandler(runtime: *mut JSRuntime, handler: Option<Intr>, opaque: *mut c_void);
    fn JS_NewContext(runtime: *mut JSRuntime) -> *mut JSContext;
    fn JS_FreeContext(context: *mut JSContext);
    fn JS_FreeRuntime(runtime: *mut JSRuntime);
    fn JS_GetGlobalObject(context: *mut JSContext) -> JSValue;
    fn pocket_bridge_free_value(context: *mut JSContext, value: JSValue);
    fn JS_Eval(
        context: *mut JSContext,
        input: *const c_char,
        length: usize,
        filename: *const c_char,
        flags: i32,
    ) -> JSValue;
    fn JS_GetException(context: *mut JSContext) -> JSValue;
    fn JS_ToCStringLen2(
        context: *mut JSContext,
        length: *mut usize,
        value: JSValue,
        cesu8: i32,
    ) -> *const c_char;
    fn JS_FreeCString(context: *mut JSContext, value: *const c_char);
    fn pocket_guest_quickjs_attach(context: *mut JSContext, flags: i32) -> *mut QuickJsAdapter;
    fn pocket_guest_quickjs_vtable() -> *const Vtable;
    fn pocket_guest_quickjs_error(guest: *const QuickJsAdapter) -> *const c_char;
    fn pocket_guest_quickjs_set_interrupt_hook(
        guest: *mut QuickJsAdapter,
        callback: Option<Job>,
        opaque: *mut c_void,
    ) -> i32;
    fn pocket_guest_quickjs_destroy(guest: *mut QuickJsAdapter);
    fn JS_RunGC(runtime: *mut JSRuntime);
    fn pocket_guest_quickjs_context(guest: *mut QuickJsAdapter) -> *mut JSContext;
    fn pocket_runtime_set_guest(
        runtime: *mut PocketRuntime,
        guest: *const PocketGuestV1,
        context: *mut c_void,
    ) -> i32;
    fn pocket_runtime_boot_deferred(runtime: *mut PocketRuntime) -> i32;
    fn pocket_runtime_advance(runtime: *mut PocketRuntime) -> i32;
    fn pocket_runtime_present(runtime: *mut PocketRuntime) -> i32;
    fn pocket_runtime_complete_presentation(runtime: *mut PocketRuntime) -> i32;
}
pub type Clock = Box<dyn Fn() -> Instant>;
struct Deadline {
    until: Instant,
    active: bool,
    interrupted: bool,
    faulted: bool,
    clock: Clock,
}
impl Deadline {
    fn new(clock: Clock) -> Self {
        Self {
            until: (clock)(),
            active: false,
            interrupted: false,
            faulted: false,
            clock,
        }
    }
    fn start(&mut self, budget: Duration) {
        self.until = (self.clock)() + budget;
        self.interrupted = false;
        self.faulted = false;
        self.active = true;
    }
    fn poll_at(&mut self, now: Instant) -> bool {
        if self.active && now >= self.until {
            self.interrupted = true;
        }
        self.interrupted
    }
    fn expired(&mut self) -> bool {
        self.poll_at((self.clock)())
    }
}
fn remaining(deadline: Instant) -> Duration {
    deadline.saturating_duration_since(Instant::now())
}
unsafe fn poll(p: *mut c_void) -> i32 {
    let d = &mut *p.cast::<Deadline>();
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| d.expired())) {
        Ok(expired) => i32::from(expired),
        Err(_) => {
            d.faulted = true;
            d.interrupted = true;
            1
        }
    }
}
unsafe extern "C" fn intr(_: *mut JSRuntime, p: *mut c_void) -> i32 {
    poll(p)
}
unsafe extern "C" fn job(p: *mut c_void) -> i32 {
    poll(p)
}
unsafe fn message(p: *const c_char, f: &str) -> String {
    if p.is_null() {
        f.into()
    } else {
        CStr::from_ptr(p)
            .to_string_lossy()
            .chars()
            .take(512)
            .collect()
    }
}
unsafe fn exception(ctx: *mut JSContext) -> String {
    let v = JS_GetException(ctx);
    let p = JS_ToCStringLen2(ctx, ptr::null_mut(), v, 0);
    let e = message(p, "unprintable exception");
    if !p.is_null() {
        JS_FreeCString(ctx, p)
    }
    pocket_bridge_free_value(ctx, v);
    e
}

struct PackageBytesSource(Option<Box<[u8]>>);
impl PackageBytesSource {
    fn new(bytes: Box<[u8]>) -> Self {
        Self(Some(bytes))
    }
}
impl PackageSource for PackageBytesSource {
    fn length(&self) -> Result<usize, SourceError> {
        self.0.as_ref().map(|b| b.len()).ok_or(SourceError::Failed)
    }
    fn take_bytes(&mut self) -> Result<PackageBytes, SourceError> {
        self.0
            .take()
            .map(PackageBytes::Owned)
            .ok_or(SourceError::Failed)
    }
    fn range(&mut self, offset: usize, out: &mut [u8]) -> Result<usize, SourceError> {
        let bytes = self.0.as_ref().ok_or(SourceError::Failed)?;
        if offset > bytes.len() {
            return Err(SourceError::OutOfBounds);
        }
        let n = out.len().min(bytes.len() - offset);
        out[..n].copy_from_slice(&bytes[offset..offset + n]);
        Ok(n)
    }
}

fn extract_package_pak(package: &[u8]) -> Result<&[u8], String> {
    let guest = pocket_package::select_guest(
        package,
        "vita",
        BINDING_GENERATION,
        &[
            4, 26, 44, 169, 37, 147, 119, 165, 218, 236, 224, 12, 68, 126, 180, 136, 199, 49, 10,
            37, 14, 103, 109, 46, 190, 218, 60, 145, 106, 87, 246, 86,
        ],
        false,
    )
    .map_err(|e| format!("packaged guest admission failed: {e:?}"))?;
    Ok(guest.pak)
}
pub struct EmbeddedGuest {
    engine: *mut JSRuntime,
    context: *mut JSContext,
    global: JSValue,
    adapter: *mut QuickJsAdapter,
    runtime: *mut PocketRuntime,
    deadline: Box<Deadline>,
    state: GuestState,
    pak: Option<Box<[u8]>>,
    _package_reservation: Option<crate::adapters::AllocationReservation>,
    _pak_reservation: Option<crate::adapters::AllocationReservation>,
    allocations: crate::adapters::AllocationBudget,
    output: Option<crate::adapters::RasterOutput>,
    input: crate::adapters::InputQueue,
}
impl EmbeddedGuest {
    pub fn new(w: u32, h: u32) -> Result<Self, String> {
        Self::new_with_clock(w, h, Box::new(Instant::now))
    }

    pub fn new_with_clock(w: u32, h: u32, clock: Clock) -> Result<Self, String> {
        unsafe {
            if w == 0 || h == 0 {
                return Err("viewport dimensions must be positive".into());
            }
            let raster_width = w.checked_mul(2).ok_or("viewport overflow")?;
            let raster_height = h.checked_mul(2).ok_or("viewport overflow")?;
            let raster_bytes = crate::adapters::raster_len(raster_width, raster_height)?;
            let budget_bytes = raster_bytes
                .checked_mul(2)
                .ok_or("framebuffer size overflow")?;
            if budget_bytes > crate::adapters::M0_ALLOCATION_LIMIT {
                return Err("raster backing exceeds M0 allocation budget".into());
            }
            let frame_buffer_budget = (w as usize)
                .checked_mul(h as usize)
                .and_then(|n| n.checked_mul(16))
                .ok_or("framebuffer size overflow")?;
            let engine = JS_NewRuntime();
            if engine.is_null() {
                return Err("QuickJS runtime allocation failed".into());
            }
            JS_SetMemoryLimit(engine, GUEST_HEAP_LIMIT);
            let context = JS_NewContext(engine);
            if context.is_null() {
                JS_FreeRuntime(engine);
                return Err("QuickJS context allocation failed".into());
            }
            let adapter = pocket_guest_quickjs_attach(context, 0);
            if adapter.is_null() {
                JS_FreeContext(context);
                JS_FreeRuntime(engine);
                return Err("PocketJS adapter allocation failed".into());
            }
            let mut s = Self {
                engine,
                context,
                global: JS_GetGlobalObject(context),
                adapter,
                runtime: ptr::null_mut(),
                deadline: Box::new(Deadline::new(clock)),
                state: GuestState::Active,
                pak: None,
                _package_reservation: None,
                _pak_reservation: None,
                allocations: crate::adapters::AllocationBudget::default(),
                output: None,
                input: crate::adapters::InputQueue::default(),
            };
            let opaque = (&mut *s.deadline as *mut Deadline).cast();
            JS_SetInterruptHandler(engine, Some(intr), opaque);
            if pocket_guest_quickjs_set_interrupt_hook(adapter, Some(job), opaque) != 0 {
                s.dispose();
                return Err("job interrupt hook setup failed".into());
            }
            let runtime = match embedding::create(RuntimeConfig {
                profile_hash: [
                    4, 26, 44, 169, 37, 147, 119, 165, 218, 236, 224, 12, 68, 126, 180, 136, 199,
                    49, 10, 37, 14, 103, 109, 46, 190, 218, 60, 145, 106, 87, 246, 86,
                ],
                host_abi: BINDING_GENERATION,
                heap_limit: GUEST_HEAP_LIMIT,
                frame_buffer_budget: frame_buffer_budget.min(budget_bytes),
                profile: Profile {
                    id: Cow::Borrowed("vita"),
                    tick_hz: 60,
                    viewport: [w * 2, h * 2],
                    logical_viewports: Cow::Owned(vec![[480, 272]]),
                    dynamic_viewport: None,
                    auxiliary: None,
                    auxiliary_logical: Cow::Borrowed(&[]),
                    density: 2,
                    required_inputs: pocket_drivers::InputClasses::NONE,
                },
            }) {
                Ok(r) => r,
                Err(e) => {
                    s.dispose();
                    return Err(format!("PocketJS runtime creation failed: {e:?}"));
                }
            };
            s.runtime = runtime;
            let package_bytes = include_bytes!("../../../pocket-ui/dist/timeline-probe.pocket");
            s._package_reservation = Some(s.allocations.reserve(package_bytes.len())?);
            let mut package = Vec::new();
            package
                .try_reserve_exact(package_bytes.len())
                .map_err(|_| "package allocation failed")?;
            package.extend_from_slice(package_bytes);
            let package = package.into_boxed_slice();
            let pak_slice = extract_package_pak(&package)?;
            s._pak_reservation = Some(s.allocations.reserve(pak_slice.len())?);
            let mut pak = Vec::new();
            pak.try_reserve_exact(pak_slice.len())
                .map_err(|_| "guest package allocation failed")?;
            pak.extend_from_slice(pak_slice);
            let pak = pak.into_boxed_slice();
            let output = crate::adapters::RasterOutput::new_bounded(
                w.checked_mul(2).ok_or("viewport overflow")?,
                h.checked_mul(2).ok_or("viewport overflow")?,
                &s.allocations,
            )?;
            s.context = pocket_guest_quickjs_context(adapter);
            let input = s.input.clone();
            let package_status =
                embedding::set_package_source(runtime, Box::new(PackageBytesSource::new(package)));
            if package_status != POCKET_STATUS_OK {
                s.dispose();
                return Err(format!("package registration failed: {package_status}"));
            }
            let input_status = embedding::add_input_source(runtime, Box::new(input.clone()));
            if input_status != POCKET_STATUS_OK {
                s.dispose();
                return Err(format!("input registration failed: {input_status}"));
            }
            let renderer = pocket_runtime::RendererId::SOFTWARE;
            s.output = Some(output);
            s.pak = Some(pak);
            let display_output = s.output.as_ref().unwrap().try_clone()?;
            if embedding::add_display(runtime, display_output, renderer) != 0 {
                s.dispose();
                return Err("PocketJS display registration failed".into());
            }
            let guest = PocketGuestV1 {
                struct_size: std::mem::size_of::<PocketGuestV1>(),
                evaluate: Some(eval_guest),
                frame: Some(frame_guest),
                collect: Some(collect_guest),
                shutdown: Some(shutdown_guest),
                initialize: Some(initialize_guest),
            };
            let status = pocket_runtime_set_guest(runtime, &guest, adapter.cast());
            if status != POCKET_STATUS_OK {
                s.dispose();
                return Err(format!("PocketJS guest registration failed: {status}"));
            }
            s.deadline.start(startup_budget());
            let boot_status = pocket_runtime_boot_deferred(runtime);
            let boot_expired = s.deadline.expired();
            s.deadline.active = false;
            if boot_status != POCKET_STATUS_OK || boot_expired {
                let error = message(
                    pocket_guest_quickjs_error(s.adapter),
                    "guest initialization or boot failed",
                );
                s.dispose();
                return Err(format!(
                    "packaged guest boot failed: {boot_status}: {error}"
                ));
            }
            let present_status = pocket_runtime_present(runtime);
            if present_status != POCKET_STATUS_OK {
                s.dispose();
                return Err(format!(
                    "initial raster presentation failed: {present_status}"
                ));
            }
            Ok(s)
        }
    }
    pub fn eval(&mut self, source: &str) -> Result<(), String> {
        let source = CString::new(source).map_err(|_| "source contains NUL".to_string())?;
        let ctx = self.context;
        let adapter = self.adapter;
        let result = self.bounded(frame_budget(), move || unsafe {
            let v = JS_Eval(
                ctx,
                source.as_ptr(),
                source.as_bytes().len(),
                c"guest".as_ptr(),
                0,
            );
            if v.tag == 6 {
                Err(exception(ctx))
            } else {
                pocket_bridge_free_value(ctx, v);
                if ((*pocket_guest_quickjs_vtable()).collect.unwrap())(adapter) != 0 {
                    return Err(message(pocket_guest_quickjs_error(adapter), "job failed"));
                }
                Ok(())
            }
        });
        result
    }
    pub fn eval_global(&mut self, source: &str) -> Result<String, String> {
        self.eval_global_with_budget(source, frame_budget())
    }
    pub fn eval_global_with_deadline(
        &mut self,
        source: &str,
        deadline: Instant,
    ) -> Result<String, String> {
        self.eval_global_with_budget(source, remaining(deadline))
    }
    pub fn set_global_json(
        &mut self,
        name: &str,
        json: &str,
        deadline: Instant,
    ) -> Result<(), String> {
        if json.len() > crate::visible_text::MAX_MESSAGE_BYTES
            || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            return Err("invalid bounded global JSON".into());
        }
        let literal =
            serde_json::to_string(json).map_err(|_| "global JSON serialization failed")?;
        let source = format!("globalThis.{name}=JSON.parse({literal})");
        self.eval_global_with_deadline(&source, deadline)
            .map(|_| ())
    }
    fn eval_global_with_budget(
        &mut self,
        source: &str,
        budget: Duration,
    ) -> Result<String, String> {
        let source = CString::new(source).map_err(|_| "source contains NUL".to_string())?;
        let ctx = self.context;
        let adapter = self.adapter;
        let result = self.bounded(budget, move || unsafe {
            let v = JS_Eval(
                ctx,
                source.as_ptr(),
                source.as_bytes().len(),
                c"getter".as_ptr(),
                0,
            );
            if v.tag == 6 {
                return Err(exception(ctx));
            }
            let p = JS_ToCStringLen2(ctx, ptr::null_mut(), v, 0);
            pocket_bridge_free_value(ctx, v);
            if p.is_null() {
                return Err(exception(ctx));
            }
            let out = CStr::from_ptr(p)
                .to_string_lossy()
                .chars()
                .take(crate::MAX_NATIVE_COMMAND_BYTES)
                .collect();
            JS_FreeCString(ctx, p);
            if ((*pocket_guest_quickjs_vtable()).collect.unwrap())(adapter) != 0 {
                return Err(message(pocket_guest_quickjs_error(adapter), "job failed"));
            }
            Ok(out)
        });
        result
    }
    pub fn take_feed_commands(&mut self) -> Result<Vec<String>, String> {
        self.take_feed_commands_with_deadline(Instant::now() + frame_budget())
    }
    pub fn take_feed_commands_with_deadline(
        &mut self,
        deadline: Instant,
    ) -> Result<Vec<String>, String> {
        let raw = self.eval_global_with_budget(
            "globalThis.__bskyTakeFeedCommands ? globalThis.__bskyTakeFeedCommands() : '[]'",
            remaining(deadline),
        )?;
        if raw.len() > crate::feed::MAX_COMMAND_BYTES {
            return Err("feed commands exceed limit".into());
        }
        let values: Vec<String> =
            serde_json::from_str(&raw).map_err(|_| "invalid feed command batch")?;
        if values.len() > crate::feed::MAX_COMMANDS {
            return Err("too many feed commands".into());
        }
        for value in &values {
            crate::feed::FeedCommand::parse(value)?;
        }
        Ok(values)
    }
    pub fn deliver_status(&mut self, message: &str) -> Result<(), String> {
        self.deliver_status_with_deadline(message, Instant::now() + frame_budget())
    }
    pub fn deliver_status_with_deadline(
        &mut self,
        message: &str,
        deadline: Instant,
    ) -> Result<(), String> {
        let safe = serde_json::to_string(message).map_err(|_| "status serialization failed")?;
        self.eval_global_with_budget(
            &format!("globalThis.__bskySetFeedError?.({safe})"),
            remaining(deadline),
        )
        .map(|_| ())
    }
    pub fn deliver_feed_json(&mut self, json: &str) -> Result<(), String> {
        self.deliver_feed_json_with_deadline(json, Instant::now() + frame_budget())
    }
    pub fn deliver_feed_json_with_deadline(
        &mut self,
        json: &str,
        deadline: Instant,
    ) -> Result<(), String> {
        if json.len() > crate::feed::MAX_DELIVERY_BYTES {
            return Err("feed delivery exceeds limit".into());
        }
        let value: serde_json::Value =
            serde_json::from_str(json).map_err(|_| "invalid feed delivery")?;
        let validated = crate::feed::validate_delivery(value)?;
        let encoded = serde_json::to_string(&validated).map_err(|_| "feed serialization failed")?;
        let literal = serde_json::to_string(&encoded).map_err(|_| "feed serialization failed")?;
        let source = CString::new(format!("globalThis.__bskyReceiveFeed({literal})"))
            .map_err(|_| "feed source contains NUL")?;
        let ctx = self.context;
        self.bounded(remaining(deadline), move || unsafe {
            let value = JS_Eval(
                ctx,
                source.as_ptr(),
                source.as_bytes().len(),
                c"feed".as_ptr(),
                0,
            );
            if value.tag == 6 {
                return Err(exception(ctx));
            }
            pocket_bridge_free_value(ctx, value);
            Ok(())
        })
    }
    pub fn frame(&mut self, buttons: u32) -> Result<(), String> {
        self.frame_with_deadline(buttons, Instant::now() + frame_budget())
    }
    pub fn frame_with_deadline(&mut self, buttons: u32, deadline: Instant) -> Result<(), String> {
        self.advance_with_deadline(buttons, deadline)?;
        self.present_with_deadline(deadline)
    }
    pub fn frame_with_budget(&mut self, buttons: u32, budget: Duration) -> Result<(), String> {
        self.advance_with_budget(buttons, budget)?;
        self.present_with_budget(budget)
    }
    pub fn advance_with_deadline(&mut self, buttons: u32, deadline: Instant) -> Result<(), String> {
        self.advance_with_budget(buttons, remaining(deadline))
    }
    pub fn advance_with_budget(&mut self, buttons: u32, budget: Duration) -> Result<(), String> {
        self.input.set_buttons(buttons);
        let runtime = self.runtime;
        self.bounded(budget, move || unsafe {
            if pocket_runtime_advance(runtime) != POCKET_STATUS_OK {
                return Err("runtime frame execution failed".into());
            }
            Ok(())
        })
    }
    pub fn present_with_deadline(&mut self, deadline: Instant) -> Result<(), String> {
        self.present_with_budget(remaining(deadline))
    }
    pub fn present_with_budget(&mut self, budget: Duration) -> Result<(), String> {
        let runtime = self.runtime;
        self.bounded(budget, move || unsafe {
            if pocket_runtime_present(runtime) != POCKET_STATUS_OK {
                return Err("runtime frame presentation failed".into());
            }
            if pocket_runtime_complete_presentation(runtime) != POCKET_STATUS_OK {
                return Err("runtime raster completion failed".into());
            }
            Ok(())
        })
    }
    pub fn native_batch_with_deadline(
        &mut self,
        deadline: Instant,
    ) -> Result<crate::NativeBatch, String> {
        self.native_batch_with_budget(remaining(deadline))
    }
    pub fn native_batch(&mut self) -> Result<crate::NativeBatch, String> {
        self.native_batch_with_budget(frame_budget())
    }
    pub fn native_batch_with_budget(
        &mut self,
        budget: Duration,
    ) -> Result<crate::NativeBatch, String> {
        let value =
            self.eval_global_with_budget("JSON.stringify(globalThis.nativeBatch)", budget)?;
        crate::NativeBatch::from_json(&value, 480, 272)
    }

    pub fn measure_callback(&mut self, text: &str) -> Result<(i32, i32), String> {
        let payload = serde_json::json!({"text": text, "scale": 1.0}).to_string();
        let arg = serde_json::to_string(&payload).map_err(|_| "text serialization failed")?;
        let value = self.eval_global(&format!(
            "JSON.stringify(globalThis.__bskyNativeMeasure({arg},1))"
        ))?;
        let value: serde_json::Value =
            serde_json::from_str(&value).map_err(|_| "invalid guest measurement")?;
        let width = value
            .get("textWidth")
            .and_then(|v| v.as_f64())
            .ok_or("invalid measured width")?;
        let height = value
            .get("textHeight")
            .and_then(|v| v.as_f64())
            .ok_or("invalid measured height")?;
        Ok((width.round() as i32, height.round() as i32))
    }

    pub fn deliver_text_metrics(
        &mut self,
        text: &str,
        width: i32,
        height: i32,
    ) -> Result<(), String> {
        self.deliver_text_metrics_with_budget(text, width, height, frame_budget())
    }
    pub fn deliver_text_metrics_with_budget(
        &mut self,
        text: &str,
        width: i32,
        height: i32,
        budget: Duration,
    ) -> Result<(), String> {
        if text.len() > crate::MAX_NATIVE_TEXT_BYTES
            || text.as_bytes().contains(&0)
            || width < 0
            || height < 0
        {
            return Err("invalid native text metrics".into());
        }
        let value = serde_json::json!({"width": width, "height": height}).to_string();
        let literal = serde_json::to_string(&value).map_err(|_| "metrics serialization failed")?;
        let source = CString::new(format!("globalThis.__bskyNativeSetMetrics({literal})"))
            .map_err(|_| "metrics source contains NUL")?;
        let ctx = self.context;
        self.bounded(budget, move || unsafe {
            let value = JS_Eval(
                ctx,
                source.as_ptr(),
                source.as_bytes().len(),
                c"metrics".as_ptr(),
                0,
            );
            if value.tag == 6 {
                return Err(exception(ctx));
            }
            pocket_bridge_free_value(ctx, value);
            Ok(())
        })
    }

    pub fn set_native_metrics(&mut self, width: i32, height: i32) -> Result<(), String> {
        self.deliver_text_metrics("Native measured text", width, height)
    }

    pub fn pixels(&self) -> Result<crate::adapters::OwnedPixels, String> {
        let pixels = self
            .output
            .as_ref()
            .ok_or_else(|| "guest raster unavailable".to_string())?
            .pixels()?;
        crate::adapters::bgra_to_rgba_bounded(&pixels, &self.allocations)
    }
    pub fn set_buttons(&self, buttons: u32) {
        self.input.set_buttons(buttons);
    }
    pub fn set_touch_snapshot(
        &mut self,
        snapshot: crate::TouchSnapshot,
        deadline: Instant,
    ) -> Result<(), String> {
        let contacts: Vec<_> = snapshot
            .as_slice()
            .iter()
            .map(|contact| serde_json::json!({"id":contact.id,"x":contact.x,"y":contact.y}))
            .collect();
        let json = serde_json::json!({"contacts":contacts}).to_string();
        self.set_global_json("__bskyTouch", &json, deadline)
    }

    pub fn collect(&mut self) -> Result<(), String> {
        if self.state != GuestState::Active {
            return Err("guest inactive".into());
        }
        let a = self.adapter;
        self.bounded(frame_budget(), move || unsafe {
            if ((*pocket_guest_quickjs_vtable()).collect.unwrap())(a) == 0 {
                Ok(())
            } else {
                Err(message(pocket_guest_quickjs_error(a), "job failed"))
            }
        })
    }
    pub fn state(&self) -> GuestState {
        self.state
    }
    pub fn dispose(&mut self) {
        unsafe {
            if self.state == GuestState::Disposed {
                return;
            }
            if !self.runtime.is_null() {
                pocket_runtime_destroy(self.runtime);
                self.runtime = ptr::null_mut();
            }
            if !self.adapter.is_null() {
                pocket_guest_quickjs_destroy(self.adapter);
                self.adapter = ptr::null_mut()
            }
            if !self.context.is_null() {
                pocket_bridge_free_value(self.context, self.global);
                JS_FreeContext(self.context);
                self.context = ptr::null_mut()
            }
            if !self.engine.is_null() {
                JS_RunGC(self.engine);
                JS_FreeRuntime(self.engine);
                self.engine = ptr::null_mut()
            }
            self.state = GuestState::Disposed
        }
    }
    pub fn bounded<T>(
        &mut self,
        b: Duration,
        f: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        if self.state != GuestState::Active {
            return Err("guest inactive".into());
        }
        self.deadline.start(b);
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        let callback_faulted = self.deadline.faulted;
        let expired = if callback_faulted {
            true
        } else {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.deadline.expired()))
                .unwrap_or(true)
        };
        self.deadline.active = false;
        if callback_faulted {
            self.state = GuestState::Failed;
            self.dispose();
            return Err("guest deadline clock failed".into());
        }
        match (r, expired) {
            (Ok(Ok(v)), false) => Ok(v),
            (Ok(Err(_)), true) => {
                self.state = GuestState::Failed;
                self.dispose();
                Err("guest deadline exceeded".into())
            }
            (Ok(Err(e)), false) => {
                self.state = GuestState::Failed;
                self.dispose();
                Err(e)
            }
            (Err(_), _) => {
                self.state = GuestState::Failed;
                self.dispose();
                Err("host panic contained".into())
            }
            (_, true) => {
                self.state = GuestState::Failed;
                self.dispose();
                Err("guest deadline exceeded".into())
            }
        }
    }
}
unsafe extern "C" fn eval_guest(
    c: *mut c_void,
    r: *mut PocketRuntime,
    code: *const u8,
    n: usize,
) -> i32 {
    let a = c.cast::<QuickJsAdapter>();
    ((*pocket_guest_quickjs_vtable()).evaluate.unwrap())(a, r, code, n)
}
unsafe extern "C" fn frame_guest(c: *mut c_void, f: *const PocketGuestFrame) -> i32 {
    ((*pocket_guest_quickjs_vtable()).frame.unwrap())(c.cast(), f.cast())
}
unsafe extern "C" fn collect_guest(c: *mut c_void) -> i32 {
    ((*pocket_guest_quickjs_vtable()).collect.unwrap())(c.cast())
}
unsafe extern "C" fn initialize_guest(
    c: *mut c_void,
    r: *mut PocketRuntime,
    environment: *const PocketGuestEnvironment,
) -> i32 {
    ((*pocket_guest_quickjs_vtable()).initialize.unwrap())(c.cast(), r, environment)
}
unsafe extern "C" fn shutdown_guest(c: *mut c_void) {
    if let Some(f) = (*pocket_guest_quickjs_vtable()).shutdown {
        f(c.cast())
    }
}
impl Drop for EmbeddedGuest {
    fn drop(&mut self) {
        self.dispose()
    }
}
