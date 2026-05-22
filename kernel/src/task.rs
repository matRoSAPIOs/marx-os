#![allow(dead_code)] // dormant since Phase 7.3.3 (desktop took over main loop)

//! Preemptive round-robin scheduler.
//!
//! Each task owns a heap-allocated stack plus a saved `RSP`. A switch is two
//! steps:
//!   1. `yield_now()` (called manually OR from the timer IRQ) locks the
//!      scheduler briefly, picks the next task, then drops the lock.
//!   2. `context_switch()` (raw asm) pushes the current task's callee-saved
//!      regs + RFLAGS onto its own stack, writes the new RSP into the old
//!      task's slot, loads the new task's RSP, pops its regs + RFLAGS, `ret`s.
//!
//! New tasks have their stacks primed to look like they're about to `ret`
//! into their entry function with `RFLAGS.IF = 1`, so they wake up with
//! interrupts already enabled.

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

/// 64 KiB per task. Generous for a hobby kernel; cuts later if we ever spawn
/// hundreds of tasks (each costs a full heap-page chunk).
const STACK_SIZE: usize = 4096 * 16;

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

pub struct Task {
    pub id: u64,
    /// Saved stack pointer for this task when it is **not** running.
    rsp: u64,
    /// Owns the backing stack memory; never accessed directly after init.
    _stack: Box<[u8]>,
}

impl Task {
    /// Placeholder Task for the currently-executing context (e.g. `kernel_main`).
    /// Its `rsp` is zero and gets written by the first `context_switch` that
    /// switches *away* from it.
    fn placeholder_for_current() -> Self {
        Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            rsp: 0,
            _stack: Vec::new().into_boxed_slice(),
        }
    }

    /// Build a fresh task that will jump to `entry` on its first scheduled run.
    pub fn new(entry: fn() -> !) -> Self {
        // Same as `new_with_arg` but with zero in RDI — callers that don't
        // need an argument can ignore it.
        Self::new_with_arg(entry as u64, 0)
    }

    /// Build a fresh task whose entry is a raw function pointer (for code
    /// loaded at runtime from disk, e.g. ELF apps). The pointer is called
    /// with `arg` placed in RDI per the System V AMD64 ABI.
    pub fn new_with_arg(entry: u64, arg: u64) -> Self {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let mut stack = vec![0u8; STACK_SIZE].into_boxed_slice();

        let stack_ptr = stack.as_mut_ptr() as usize;
        let stack_top_raw = stack_ptr + STACK_SIZE;
        let aligned_top = (stack_top_raw - 8) & !0xF;

        // Layout of the freshly-primed stack (low → high):
        //
        //   rsp+0   r15       (popped by context_switch)
        //   rsp+8   r14
        //   rsp+16  r13
        //   rsp+24  r12
        //   rsp+32  rbx
        //   rsp+40  rbp
        //   rsp+48  RFLAGS    (popfq → IF=1)
        //   rsp+56  RIP of stage1 trampoline (`ret` here)
        //
        // The trampoline reads `arg` and `entry` from a slot we squirrel
        // away in `r12` (callee-saved across context_switch) and jumps to
        // `entry` with `arg` in RDI.  Simpler: we just store arg in a
        // dedicated callee-saved register (r12) and entry in r13, then
        // the trampoline does `mov rdi, r12; jmp r13`.
        let initial_rsp = aligned_top - 56;

        unsafe {
            *(aligned_top         as *mut u64) = task_trampoline as *const () as u64; // RIP for RET
            *((aligned_top -  8)  as *mut u64) = 0x202;                  // RFLAGS: IF=1
            *((aligned_top - 16)  as *mut u64) = 0;                      // rbp
            *((aligned_top - 24)  as *mut u64) = 0;                      // rbx
            *((aligned_top - 32)  as *mut u64) = entry;                  // r12 -> trampoline reads this as entry
            *((aligned_top - 40)  as *mut u64) = arg;                    // r13 -> trampoline reads this as arg
            *((aligned_top - 48)  as *mut u64) = 0;                      // r14
            *((aligned_top - 56)  as *mut u64) = 0;                      // r15
        }

        Self { id, rsp: initial_rsp as u64, _stack: stack }
    }
}

struct Scheduler {
    /// Boxed so that `Vec::push` re-alloc doesn't move the `rsp` field of any
    /// existing task (and invalidate raw pointers stashed by `yield_now`).
    tasks: Vec<Box<Task>>,
    current: usize,
}

static SCHEDULER: Mutex<Option<Scheduler>> = Mutex::new(None);

/// Stand up the scheduler with the currently-running context as task 0.
pub fn init() {
    let placeholder = Task::placeholder_for_current();
    let mut sched = SCHEDULER.lock();
    assert!(sched.is_none(), "task::init called twice");
    *sched = Some(Scheduler {
        tasks: vec![Box::new(placeholder)],
        current: 0,
    });
}

/// Spawn a new task; returns its ID. Safe to call from task or IRQ context.
pub fn spawn(entry: fn() -> !) -> u64 {
    use x86_64::instructions::interrupts;
    let task = Box::new(Task::new(entry));
    let id = task.id;
    interrupts::without_interrupts(|| {
        SCHEDULER
            .lock()
            .as_mut()
            .expect("task::init not called")
            .tasks
            .push(task);
    });
    id
}

/// Forcibly release the scheduler mutex. Reserved for soft-recovery / "kill task" paths.
#[allow(dead_code)]
pub unsafe fn panic_unlock() {
    SCHEDULER.force_unlock();
}

/// Drop the currently-running task and context-switch into `target_id`. Never
/// returns. The dying task's stack and Task struct are intentionally leaked
/// (we're still executing on that stack — freeing it would be a use-after-free).
///
/// Reserved for a future "kill task" / soft-recovery feature.
#[allow(dead_code)]
pub fn abandon_current_switch_to(target_id: u64) -> ! {
    use x86_64::instructions::interrupts;

    // The caller (panic_handler) already disabled IRQs, but make sure.
    interrupts::disable();

    let next_rsp: u64 = {
        let mut guard = SCHEDULER.lock();
        let sched = guard.as_mut().expect("scheduler not initialised");

        let target_idx = sched
            .tasks
            .iter()
            .position(|t| t.id == target_id)
            .expect("abandon target not found");

        let current = sched.current;
        assert!(target_idx != current, "abandon target == current");

        // Capture the destination RSP BEFORE we mutate the Vec.
        let target_rsp = sched.tasks[target_idx].rsp;

        // Remove the dying task and leak it — we're standing on its stack.
        let removed = sched.tasks.remove(current);
        core::mem::forget(removed);

        // remove() shifted everything after `current` down by one.
        let new_target_idx =
            if target_idx > current { target_idx - 1 } else { target_idx };
        sched.current = new_target_idx;

        target_rsp
    };

    // One-way switch: the value written through `_dummy_rsp` lands on the
    // dying stack and nobody will ever read it.
    let mut _dummy_rsp: u64 = 0;
    unsafe { context_switch(&mut _dummy_rsp as *mut u64, next_rsp); }
    unreachable!("context_switch returned into abandoned task")
}

/// Round-robin switch to the next ready task. No-op if scheduler is empty or
/// only the bootstrap task exists. Safe to call from task code OR from a
/// hardware-interrupt handler.
pub fn yield_now() {
    use x86_64::instructions::interrupts;

    // Snapshot caller's IF state. `context_switch` will inherit whatever
    // RFLAGS we push, so we must disable IF for the duration of the swap to
    // avoid being preempted mid-switch.
    let was_enabled = interrupts::are_enabled();
    interrupts::disable();

    let (prev_rsp_ptr, next_rsp): (*mut u64, u64) = {
        let mut guard = SCHEDULER.lock();
        let sched = match guard.as_mut() {
            Some(s) => s,
            None => {
                if was_enabled { interrupts::enable(); }
                return;
            }
        };
        let n = sched.tasks.len();
        if n <= 1 {
            if was_enabled { interrupts::enable(); }
            return;
        }
        let prev = sched.current;
        let next = (prev + 1) % n;
        sched.current = next;
        // Boxed tasks → these raw pointers remain valid even if the Vec
        // reallocates between now and the next yield.
        (
            &mut sched.tasks[prev].rsp as *mut u64,
            sched.tasks[next].rsp,
        )
    };

    unsafe { context_switch(prev_rsp_ptr, next_rsp); }

    // On resume `popfq` inside context_switch restored the RFLAGS we pushed
    // (IF=0). Reinstate the caller's view of IF.
    if was_enabled { interrupts::enable(); }
}

// ---------------------------------------------------------------- raw switch

core::arch::global_asm!(
"
.global context_switch
context_switch:
    pushfq
    push rbp
    push rbx
    push r12
    push r13
    push r14
    push r15
    mov [rdi], rsp
    mov rsp, rsi
    pop r15
    pop r14
    pop r13
    pop r12
    pop rbx
    pop rbp
    popfq
    ret

.global task_trampoline
task_trampoline:
    // First time a `new_with_arg` task runs, it ends up here via `ret`.
    // We seeded its r12 slot with the real entry address and r13 with the
    // first argument.  System V ABI: first arg goes in RDI.
    mov rdi, r13
    jmp r12
"
);

extern "C" {
    /// Save the current task's callee-saved regs + RFLAGS onto its stack,
    /// write the resulting RSP through `prev_rsp_ptr`, then load `next_rsp`
    /// and restore the destination task's regs.
    fn context_switch(prev_rsp_ptr: *mut u64, next_rsp: u64);

    /// Trampoline that takes the (entry, arg) saved in r12/r13 by
    /// `Task::new_with_arg` and tail-calls entry(arg).
    fn task_trampoline() -> !;
}
