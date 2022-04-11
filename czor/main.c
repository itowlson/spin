#include <stdint.h>
#include <stdio.h>

extern int32_t upples(const char* addr, const char* app, const char* tmp, const char* log);

int main() {
    int32_t res = upples(
        "127.0.0.1:3000",
        "/home/ivan/github/spin/examples/http-rust/spin.toml",
        "./TEMPYWEMPY",
        "./LOGGLYWOGGLY"
    );
    printf("UPPLES: %d\n", res);
    return res;
}
