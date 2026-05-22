//! Single-producer / single-consumer keyboard input buffer.
//!
//! Producer: the keyboard IRQ handler (`interrupts::keyboard_handler`).
//! Consumer: the shell task (or any other code that calls `pop_blocking`).
//!
//! Backed by a fixed 256-byte ring in a `spin::Mutex`. We deliberately **do
//! not** use a `VecDeque` here — that would touch the global allocator from
//! IRQ context, and if the interrupted task happened to be inside the
//! allocator (holding its internal spin lock), we'd deadlock. Fixed array =
//! no allocator involvement, ever.

use spin::Mutex;

const CAP: usize = 256;

struct Ring {
    buf: [u8; CAP],
    head: usize, // write index
    tail: usize, // read index
}

impl Ring {
    const fn new() -> Self {
        Self { buf: [0; CAP], head: 0, tail: 0 }
    }

    fn push(&mut self, b: u8) -> bool {
        let next = (self.head + 1) % CAP;
        if next == self.tail { return false; } // full — drop oldest write loss
        self.buf[self.head] = b;
        self.head = next;
        true
    }

    fn pop(&mut self) -> Option<u8> {
        if self.tail == self.head { return None; } // empty
        let b = self.buf[self.tail];
        self.tail = (self.tail + 1) % CAP;
        Some(b)
    }

    fn clear(&mut self) {
        self.tail = self.head;
    }
}

static INPUT: Mutex<Ring> = Mutex::new(Ring::new());

/// Append a byte. Called from IRQ context. Silently drops if buffer is full.
pub fn push(b: u8) {
    // IRQ context has IF=0; the lock can never be held by anyone else, so
    // this `lock()` is effectively uncontended.
    let _ = INPUT.lock().push(b);
}

/// Discard any buffered keystrokes. Reserved for soft-recovery / "kill task" paths.
#[allow(dead_code)]
pub fn drain() {
    INPUT.lock().clear();
}

/// Non-blocking pop. Returns `None` if the ring is empty.
pub fn try_pop() -> Option<u8> {
    INPUT.lock().pop()
}

/// Forcibly release the input ring's mutex. See `framebuffer::panic_unlock`.
#[allow(dead_code)]
pub unsafe fn panic_unlock() {
    INPUT.force_unlock();
}

/// Block (via `hlt`) until at least one byte is available, then return it.
/// Cooperates with the scheduler — while we're halted, timer IRQs preempt us
/// and let other tasks run.
pub fn pop_blocking() -> u8 {
    use x86_64::instructions::interrupts;
    loop {
        // Disable IF so the check + sleep is race-free against the keyboard IRQ.
        interrupts::disable();
        if let Some(b) = INPUT.lock().pop() {
            interrupts::enable();
            return b;
        }
        // Atomic sti+hlt — guarantees no IRQ slips in between the empty-check
        // and the halt. Next IRQ wakes us.
        interrupts::enable_and_hlt();
    }
}
