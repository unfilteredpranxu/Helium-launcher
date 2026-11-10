//! Caller-owned output bridge; see helium_metal_prototype.h for the C contract.
use crate::{Image, SIDE};
use std::{ffi::c_char, panic::UnwindSafe, ptr};

const RGBA_BYTES: usize = SIDE * SIDE * 4;
const INVALID_ARGUMENT: i32 = 3;

/// # Safety
/// Both buffers must be live, exclusively writable allocations of the stated
/// capacities throughout this blocking call. They must not overlap.
#[cfg_attr(any(target_os = "macos", target_os = "ios"), unsafe(no_mangle))]
pub unsafe extern "C" fn helium_metal_smoke_test_rgba(
    rgba_out: *mut u8,
    rgba_capacity: usize,
    message_out: *mut c_char,
    message_capacity: usize,
) -> i32 {
    // SAFETY: the external caller supplies the documented allocation guarantees.
    unsafe {
        run_with(
            rgba_out,
            rgba_capacity,
            message_out.cast(),
            message_capacity,
            crate::smoke_test,
        )
    }
}

fn region(pointer: *mut u8, capacity: usize) -> Option<(usize, usize)> {
    if pointer.is_null() || capacity == 0 || capacity > isize::MAX as usize {
        return None;
    }
    Some((pointer.addr(), pointer.addr().checked_add(capacity)?))
}

// SAFETY contract: destination is valid for capacity >= 1 bytes, exclusively
// writable and disjoint from message. This helper never allocates or panics.
unsafe fn write_message(destination: *mut u8, capacity: usize, message: &str) {
    let mut count = message.len().min(capacity - 1);
    while !message.is_char_boundary(count) {
        count -= 1;
    }
    // SAFETY: count + NUL fits the validated allocation; source is a live str.
    unsafe {
        ptr::copy_nonoverlapping(message.as_ptr(), destination, count);
        destination.add(count).write(0);
    }
}

// The injected operation exists only to exercise output/exception handling in
// CPU unit tests. The exported entry always invokes the real crate::smoke_test.
unsafe fn run_with(
    rgba_out: *mut u8,
    rgba_capacity: usize,
    message_out: *mut u8,
    message_capacity: usize,
    run: impl FnOnce() -> Result<Image, String> + UnwindSafe,
) -> i32 {
    let Some((message_start, message_end)) = region(message_out, message_capacity) else {
        return INVALID_ARGUMENT;
    };
    let pixels = region(rgba_out, rgba_capacity);
    if let Some((start, end)) = pixels {
        if start < message_end && message_start < end {
            return INVALID_ARGUMENT;
        }
    }
    if pixels.is_none() || rgba_capacity < RGBA_BYTES {
        // SAFETY: the message allocation was validated and no overlapping writes occur.
        unsafe { write_message(message_out, message_capacity, "invalid RGBA output buffer") };
        return INVALID_ARGUMENT;
    }
    let outcome = std::panic::catch_unwind(|| -> Result<(), String> {
        let image = run()?;
        if image.rgba.len() != RGBA_BYTES {
            return Err("unexpected GPU readback length".into());
        }
        let message = match image.gpu_seconds {
            Some(seconds) => format!(
                "Smoke checks passed; GPU readback completed. First offscreen render+blit command buffer (single measurement): {seconds:.6} s."
            ),
            None => {
                "Smoke checks passed; GPU readback completed. GPU timestamp unavailable.".into()
            }
        };
        // SAFETY: both output regions are validated/disjoint, and image owns exactly
        // RGBA_BYTES initialized bytes. No output is written before success/formatting.
        unsafe {
            ptr::copy_nonoverlapping(image.rgba.as_ptr(), rgba_out, RGBA_BYTES);
            write_message(message_out, message_capacity, &message);
        }
        Ok(())
    });
    match outcome {
        Ok(Ok(())) => 0,
        Ok(Err(error)) => {
            // SAFETY: same valid message allocation; pixels remain untouched.
            unsafe { write_message(message_out, message_capacity, &error) };
            1
        }
        Err(_) => {
            // SAFETY: same valid message allocation; no fallible formatting here.
            unsafe {
                write_message(
                    message_out,
                    message_capacity,
                    "Rust panic during Metal test",
                )
            };
            2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CStr;

    fn message(bytes: &[u8]) -> &str {
        CStr::from_bytes_until_nul(bytes).unwrap().to_str().unwrap()
    }

    #[test]
    fn invalid_buffers_are_rejected_without_running_the_test() {
        let mut pixels = vec![0x5a; RGBA_BYTES];
        let mut text = [0x5a; 64];
        for (pointer, capacity) in [(ptr::null_mut(), RGBA_BYTES), (pixels.as_mut_ptr(), 1)] {
            // SAFETY: null/short pixel arguments are explicitly supported rejection cases.
            let status = unsafe {
                run_with(pointer, capacity, text.as_mut_ptr(), text.len(), || {
                    panic!("must not run")
                })
            };
            assert_eq!(status, 3);
            assert_eq!(message(&text), "invalid RGBA output buffer");
        }
        for (pointer, capacity) in [(ptr::null_mut(), 64), (text.as_mut_ptr(), 0)] {
            // SAFETY: invalid message arguments must be rejected without writes.
            assert_eq!(
                unsafe {
                    run_with(pixels.as_mut_ptr(), pixels.len(), pointer, capacity, || {
                        panic!("must not run")
                    })
                },
                3
            );
        }
        // SAFETY: overlapping live buffers must be rejected without writes.
        assert_eq!(
            unsafe {
                run_with(
                    pixels.as_mut_ptr(),
                    pixels.len(),
                    pixels.as_mut_ptr(),
                    64,
                    || panic!("must not run"),
                )
            },
            3
        );
        assert!(pixels.iter().all(|&b| b == 0x5a));
    }

    #[test]
    fn error_and_panic_leave_pixels_untouched_and_report_failure() {
        let mut pixels = vec![0x5a; RGBA_BYTES];
        let mut text = [0u8; 64];
        for panic in [false, true] {
            // SAFETY: separate writable arrays remain live throughout the call.
            let status = unsafe {
                run_with(
                    pixels.as_mut_ptr(),
                    pixels.len(),
                    text.as_mut_ptr(),
                    text.len(),
                    || {
                        if panic {
                            panic!("test panic")
                        }
                        Err("MSL compilation failed: test diagnostic".into())
                    },
                )
            };
            assert_eq!(status, if panic { 2 } else { 1 });
            assert!(message(&text).contains(if panic {
                "Rust panic"
            } else {
                "MSL compilation failed"
            }));
            assert!(pixels.iter().all(|&b| b == 0x5a));
        }
    }

    #[test]
    fn bridge_copies_operation_pixels_and_preserves_capacity_guards() {
        // Synthetic data tests marshalling only; no substitute image reaches the app.
        let expected: Vec<u8> = (0..RGBA_BYTES).map(|i| (i % 251) as u8).collect();
        let mut pixels = vec![0x5a; RGBA_BYTES + 2];
        let mut text = [0x5a; 256];
        // SAFETY: interior ranges are live and disjoint, including their capacities.
        let status = unsafe {
            run_with(
                pixels.as_mut_ptr().add(1),
                RGBA_BYTES,
                text.as_mut_ptr(),
                text.len() - 1,
                || {
                    Ok(Image {
                        rgba: expected.clone(),
                        gpu_seconds: Some(0.125),
                    })
                },
            )
        };
        assert_eq!(status, 0);
        assert_eq!(pixels[1..=RGBA_BYTES], expected);
        assert_eq!(
            (pixels[0], pixels[RGBA_BYTES + 1], text[255]),
            (0x5a, 0x5a, 0x5a)
        );
        assert!(message(&text).contains("0.125000 s"));
    }

    #[test]
    fn diagnostic_truncation_keeps_utf8_and_nul_within_capacity() {
        let mut pixels = vec![0u8; RGBA_BYTES];
        let mut text = [0x5a; 5];
        // SAFETY: valid disjoint arrays, with one byte beyond the message capacity as a guard.
        assert_eq!(
            unsafe {
                run_with(
                    pixels.as_mut_ptr(),
                    pixels.len(),
                    text.as_mut_ptr(),
                    4,
                    || Err("éé".into()),
                )
            },
            1
        );
        assert_eq!(message(&text), "é");
        assert_eq!(text[4], 0x5a);
    }

    #[test]
    fn malformed_readback_is_rejected_before_copying() {
        let mut pixels = vec![0x5a; RGBA_BYTES];
        let mut text = [0u8; 64];
        for length in [0, RGBA_BYTES + 1] {
            // SAFETY: valid disjoint arrays. The injected malformed image tests
            // the length guard before the unsafe fixed-size copy.
            let status = unsafe {
                run_with(
                    pixels.as_mut_ptr(),
                    pixels.len(),
                    text.as_mut_ptr(),
                    text.len(),
                    || {
                        Ok(Image {
                            rgba: vec![0; length],
                            gpu_seconds: None,
                        })
                    },
                )
            };
            assert_eq!(status, 1);
            assert_eq!(message(&text), "unexpected GPU readback length");
            assert!(pixels.iter().all(|&b| b == 0x5a));
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    #[test]
    fn exported_bridge_reports_real_unsupported_platform_failure() {
        let mut pixels = vec![0x5a; RGBA_BYTES];
        let mut text = [0u8; 128];
        // SAFETY: valid disjoint arrays for the actual public entry point.
        assert_eq!(
            unsafe {
                helium_metal_smoke_test_rgba(
                    pixels.as_mut_ptr(),
                    pixels.len(),
                    text.as_mut_ptr().cast(),
                    text.len(),
                )
            },
            1
        );
        assert_eq!(message(&text), "direct Metal requires macOS or iOS");
        assert!(pixels.iter().all(|&b| b == 0x5a));
    }
}
