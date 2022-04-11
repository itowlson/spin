ifeq ($(shell uname),Darwin)
    LDFLAGS := -Wl,-dead_strip
else
    LDFLAGS := -Wl,--gc-sections -lpthread -ldl -lssl -lcrypto -lm
endif

all: target/spinc
	target/spinc

target:
	mkdir -p $@

target/spinc: target/main.o target/debug/libspin.a
	$(CC) -o $@ $^ $(LDFLAGS)

target/debug/libspin.a: src/lib.rs src/commands/up.rs Cargo.toml
	cargo build

target/main.o: czor/main.c | target
	$(CC) -o $@ -c $<

clean:
	rm -rf target