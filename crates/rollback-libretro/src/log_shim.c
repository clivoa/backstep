/* The libretro log interface, which stable Rust cannot express.
 *
 * `retro_log_printf_t` is `void (*)(enum retro_log_level, const char *fmt, ...)`
 * -- a C-variadic function. Stable Rust can *call* one but cannot *define* one,
 * so the frontend has to hand the core a real C function.
 *
 * This matters more than it sounds. FBNeo reports a missing or wrong ROM file
 * exclusively through this channel:
 *
 *     [FBNeo] ROM at index 21 with name sfa3.key and CRC 0x54fa39c6 is required
 *
 * That line never reaches SET_MESSAGE. Without this shim the same failure
 * surfaces only as `retro_serialize_size() == 0`, and finding out which file is
 * missing means comparing the zip's CRCs against the driver's romset table by
 * hand.
 *
 * The shim does nothing but format: it renders the varargs with vsnprintf and
 * hands the finished string to Rust, which owns all the buffering and locking.
 */

#include <stdarg.h>
#include <stdio.h>

/* Defined in host.rs. Takes a NUL-terminated string valid for the call only. */
void rollback_libretro_log_line(unsigned level, const char *line);

/* Long enough for FBNeo's longest diagnostic; truncation is reported, not
 * silent, so a surprise here cannot masquerade as a complete message. */
#define LOG_SHIM_BUFFER 2048

void rollback_libretro_log_shim(unsigned level, const char *fmt, ...)
{
	char buffer[LOG_SHIM_BUFFER];
	va_list args;
	int written;

	if (fmt == NULL) {
		return;
	}

	va_start(args, fmt);
	written = vsnprintf(buffer, sizeof(buffer), fmt, args);
	va_end(args);

	if (written < 0) {
		rollback_libretro_log_line(level, "<log line could not be formatted>");
		return;
	}
	if ((size_t)written >= sizeof(buffer)) {
		/* vsnprintf already NUL-terminated at the cut; say so explicitly. */
		rollback_libretro_log_line(level, buffer);
		rollback_libretro_log_line(level, "<previous line was truncated>");
		return;
	}

	rollback_libretro_log_line(level, buffer);
}
