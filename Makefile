# Local RPM build helper for leyen.
#
# Replace .local/Makefile with this file. Do not append it to the old one.
#
# Usage:
#
#   make info
#   make
#   make rpm
#   make srpm
#   make archive
#   make clean
#
# Override paths if needed:
#
#   make PROJECT_DIR=/path/to/leyen
#   make VERSION=0.1.0

SHELL := /usr/bin/env bash
.DELETE_ON_ERROR:

MAKEFILE_DIR := $(abspath $(dir $(lastword $(MAKEFILE_LIST))))

AUTO_PROJECT_DIR := $(shell \
	if [[ -f "$(MAKEFILE_DIR)/../leyen.spec" ]]; then \
		cd "$(MAKEFILE_DIR)/.." && pwd; \
	elif [[ -f "$(MAKEFILE_DIR)/../leyen/leyen.spec" ]]; then \
		cd "$(MAKEFILE_DIR)/../leyen" && pwd; \
	else \
		cd "$(MAKEFILE_DIR)/.." && pwd; \
	fi \
)

PROJECT_DIR ?= $(AUTO_PROJECT_DIR)

NAME ?= leyen
SPEC ?= $(PROJECT_DIR)/$(NAME).spec

RPMBUILD_DIR ?= $(HOME)/rpmbuild
SOURCES_DIR  ?= $(RPMBUILD_DIR)/SOURCES
SRPMS_DIR    ?= $(RPMBUILD_DIR)/SRPMS
RPMS_DIR     ?= $(RPMBUILD_DIR)/RPMS

# Version comes from the RPM spec, not Cargo.toml.
VERSION ?= $(shell \
	if [[ -f "$(SPEC)" ]]; then \
		awk 'BEGIN{IGNORECASE=1} /^[[:space:]]*Version[[:space:]]*:/ {print $$2; exit}' "$(SPEC)"; \
	fi \
)

ARCHIVE_NAME := $(NAME)-$(VERSION).tar.gz
ARCHIVE_PATH := $(SOURCES_DIR)/$(ARCHIVE_NAME)

.PHONY: all rpm ba srpm bs archive prepare clean info check

all: srpm

rpm: ba

ba: archive
	rpmbuild -ba "$(SPEC)"

srpm: bs

bs: archive
	rpmbuild -bs "$(SPEC)"

archive: check prepare
	@echo ":: creating $(ARCHIVE_PATH)"
	@tmpdir="$$(mktemp -d)"; \
	trap 'rm -rf "$$tmpdir"' EXIT; \
	mkdir -p "$$tmpdir/$(NAME)-$(VERSION)"; \
	rsync -rt --delete \
		--chmod=Du=rwx,Dgo=rx,Fu=rw,Fgo=r \
		--exclude='.git' \
		--exclude='.gitignore' \
		--exclude='.copr/results' \
		--exclude='.local' \
		--exclude='result' \
		--exclude='results' \
		--exclude='dist' \
		--exclude='build' \
		--exclude='target' \
		--exclude='__pycache__' \
		--exclude='*.pyc' \
		"$(PROJECT_DIR)/" "$$tmpdir/$(NAME)-$(VERSION)/"; \
	tar --owner=0 --group=0 --numeric-owner \
		-C "$$tmpdir" -czf "$(ARCHIVE_PATH)" "$(NAME)-$(VERSION)"
	@echo ":: source archive ready: $(ARCHIVE_PATH)"

prepare:
	@mkdir -p "$(SOURCES_DIR)" "$(SRPMS_DIR)" "$(RPMS_DIR)"

check:
	@test -d "$(PROJECT_DIR)" || { echo "ERROR: PROJECT_DIR does not exist: $(PROJECT_DIR)" >&2; exit 1; }
	@test -f "$(SPEC)" || { echo "ERROR: spec not found: $(SPEC)" >&2; exit 1; }
	@test -n "$(VERSION)" || { echo "ERROR: could not read Version: from $(SPEC)" >&2; exit 1; }
	@test -f "$(PROJECT_DIR)/Cargo.toml" || { echo "ERROR: Cargo.toml not found in $(PROJECT_DIR)" >&2; exit 1; }
	@command -v awk >/dev/null || { echo "ERROR: awk not found" >&2; exit 1; }
	@command -v rsync >/dev/null || { echo "ERROR: rsync not found" >&2; exit 1; }
	@command -v tar >/dev/null || { echo "ERROR: tar not found" >&2; exit 1; }
	@command -v rpmbuild >/dev/null || { echo "ERROR: rpmbuild not found. Install rpm-build." >&2; exit 1; }

info:
	@echo "NAME:             $(NAME)"
	@echo "VERSION:          $(VERSION)"
	@echo "MAKEFILE_DIR:     $(MAKEFILE_DIR)"
	@echo "AUTO_PROJECT_DIR: $(AUTO_PROJECT_DIR)"
	@echo "PROJECT_DIR:      $(PROJECT_DIR)"
	@echo "SPEC:             $(SPEC)"
	@echo "RPMBUILD_DIR:     $(RPMBUILD_DIR)"
	@echo "SOURCES_DIR:      $(SOURCES_DIR)"
	@echo "ARCHIVE_PATH:     $(ARCHIVE_PATH)"

clean:
	rm -f "$(ARCHIVE_PATH)"
