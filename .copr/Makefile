TOPDIR := $(HOME)/rpmbuild
SOURCES_DIR := $(TOPDIR)/SOURCES
SPEC ?= leyen.spec
NAME := leyen
VERSION := $(shell rpmspec -q --qf '%{VERSION}\n' --srpm $(SPEC) | head -n1)
outdir ?= $(CURDIR)
spec ?= $(SPEC)

all: srpm

srpm:
	mkdir -p "$(SOURCES_DIR)" "$(outdir)"
	spectool -g -R $(spec)
	rpmbuild -bs "$(SPEC)"

clean:
	rm -rf target result results .copr/results
