TOPDIR := $(HOME)/rpmbuild
SOURCES_DIR := $(TOPDIR)/SOURCES
SPEC ?= leyen.spec
NAME := leyen
VERSION := $(shell rpmspec -q --qf '%{VERSION}\n' --srpm $(SPEC) | head -n1)
VENDOR_TARBALL := $(SOURCES_DIR)/$(NAME)-$(VERSION)-vendor.tar.zst

outdir ?= $(CURDIR)
spec ?= $(SPEC)

all: srpm

srpm:
	mkdir -p "$(SOURCES_DIR)" "$(outdir)"
	tmpdir="$$(mktemp -d)"; \
	trap 'rm -rf "$$tmpdir"' EXIT; \
	cargo vendor "$$tmpdir/vendor" >/dev/null; \
	tar --zstd -C "$$tmpdir" -cf "$(VENDOR_TARBALL)" vendor
	spectool -g -R "$(spec)"
	rpmbuild -bs "$(SPEC)"

clean:
	rm -rf target result results .copr/results
