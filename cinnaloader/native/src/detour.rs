//! Inline hooking backend for x86-64, built on `ilhook`.
//!
//! A hook replaces the target function's entry so our code runs; the callback is handed
//! the original function pointer so the real body stays callable. This is how the loader
//! will intercept client functions like `queue_chat_send` once their address is resolved
//! from the symbol table.

#[cfg(target_arch = "x86_64")]
pub use ilhook::x64::{
    hook_closure_jmp_back, hook_closure_retn, CallbackOption, ClosureHookPoint, HookFlags,
    Registers,
};

#[cfg(test)]
mod tests {
    #[cfg(target_arch = "x86_64")]
    #[test]
    fn detour_intercepts_and_calls_the_original() {
        use super::{hook_closure_retn, CallbackOption, HookFlags, Registers};
        use std::sync::atomic::{AtomicBool, Ordering};

        static HIT: AtomicBool = AtomicBool::new(false);

        // Several instructions so the prologue is long enough to relocate safely.
        #[inline(never)]
        extern "C" fn target(x: i32) -> i32 {
            let mut y = x;
            y = y.wrapping_mul(3);
            y = y.wrapping_add(7);
            y ^ 0x55
        }

        // Force an indirect call so the target cannot be inlined at the call site.
        let target_fn: extern "C" fn(i32) -> i32 = target;
        let indirect = std::hint::black_box(target_fn);
        let expected_original = target(5);

        let hook = unsafe {
            hook_closure_retn(
                target_fn as usize,
                move |_regs: *mut Registers, original: usize| -> usize {
                    HIT.store(true, Ordering::SeqCst);
                    // SAFETY: `original` is the target's trampoline with the same signature.
                    let original: extern "C" fn(i32) -> i32 = std::mem::transmute(original);
                    (original(5) + 100) as usize
                },
                CallbackOption::None,
                HookFlags::empty(),
            )
        }
        .expect("install hook");

        assert_eq!(indirect(5), expected_original + 100, "hook must intercept");
        assert!(HIT.load(Ordering::SeqCst), "hook body must run");
        drop(hook);
    }
}
