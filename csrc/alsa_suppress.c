// Minimal no-op ALSA error handler to suppress noisy probe messages.
#include <stdarg.h>

void pyspeaker_silent_alsa_handler(
    const char *file,
    int line,
    const char *function,
    int err,
    const char *fmt,
    ...
) {
    // Intentionally silent
    (void)file;
    (void)line;
    (void)function;
    (void)err;
    (void)fmt;
}
