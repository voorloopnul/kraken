/* Contents/MacOS/kraken: a compiled arm64 stub, exec'ing launcher.sh (the
 * bundle's real entry point, in Contents/Resources) with the app's argv.
 *
 * It has to be a real Mach-O binary rather than the shell script directly,
 * because CFBundleExecutable is how macOS decides whether an app is native.
 * LaunchServices reads the *load commands* of that file to learn which
 * architectures it supports; a shell script has none, so a script there
 * reads as an app with no arm64 slice, and it offers to install Rosetta
 * before running something that, once running, never executes a single
 * x86_64 instruction. Give it a real arm64 binary to inspect instead and
 * the prompt never happens.
 *
 * Kept deliberately dumb: find Contents/Resources/launcher.sh next to this
 * binary's own resolved path, and exec bash on it with argv forwarded. All
 * the actual setup (PATH, env vars, the final exec of python3) stays in that
 * script, in bash, where it is easy to read and change.
 */
#include <limits.h>
#include <mach-o/dyld.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

int main(int argc, char *argv[]) {
    char exe_path[PATH_MAX];
    uint32_t size = sizeof(exe_path);
    if (_NSGetExecutablePath(exe_path, &size) != 0) {
        fprintf(stderr, "kraken: executable path too long\n");
        return 1;
    }

    char resolved[PATH_MAX];
    if (realpath(exe_path, resolved) == NULL) {
        perror("kraken: realpath");
        return 1;
    }

    /* resolved is .../Contents/MacOS/kraken; strip two path components to
     * land on .../Contents, then descend into Resources. Done with strrchr
     * rather than dirname(3), which is free to hand back a pointer into a
     * shared static buffer that a second call would overwrite. */
    char *slash = strrchr(resolved, '/'); /* .../Contents/MacOS|/kraken */
    if (slash == NULL) {
        fprintf(stderr, "kraken: unexpected executable path: %s\n", resolved);
        return 1;
    }
    *slash = '\0';
    slash = strrchr(resolved, '/'); /* .../Contents|/MacOS */
    if (slash == NULL) {
        fprintf(stderr, "kraken: unexpected executable path: %s\n", resolved);
        return 1;
    }
    *slash = '\0'; /* resolved is now .../Contents */

    char script_path[PATH_MAX];
    if ((size_t)snprintf(script_path, sizeof(script_path),
                          "%s/Resources/launcher.sh", resolved) >=
        sizeof(script_path)) {
        fprintf(stderr, "kraken: bundle path too long\n");
        return 1;
    }

    char *new_argv[argc + 2];
    new_argv[0] = "/bin/bash";
    new_argv[1] = script_path;
    for (int i = 1; i < argc; i++) {
        new_argv[i + 1] = argv[i];
    }
    new_argv[argc + 1] = NULL;

    execv("/bin/bash", new_argv);
    perror("kraken: execv"); /* only reached if execv itself failed */
    return 1;
}
