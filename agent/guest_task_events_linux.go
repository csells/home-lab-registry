//go:build linux

package main

import (
	"bytes"
	"context"
	"encoding/binary"
	"fmt"
	"log"
	"os"
	"path/filepath"

	"golang.org/x/sys/unix"
)

// guestTaskEvents wakes the collector after the Proxmox host's task database
// changes. Task entries are only invalidations; the API supplies actual state.
func guestTaskEvents(ctx context.Context, directory string) (<-chan struct{}, error) {
	fd, err := unix.InotifyInit1(unix.IN_CLOEXEC | unix.IN_NONBLOCK)
	if err != nil {
		return nil, err
	}
	file := os.NewFile(uintptr(fd), "guest-task-events")
	if _, err := unix.InotifyAddWatch(fd, directory, unix.IN_ONLYDIR|unix.IN_CLOSE_WRITE|unix.IN_MOVED_TO|unix.IN_CREATE|unix.IN_MOVE_SELF|unix.IN_DELETE_SELF); err != nil {
		file.Close()
		return nil, err
	}
	wake := make(chan struct{}, 1)
	done := make(chan struct{})
	go func() {
		select {
		case <-ctx.Done():
			file.Close()
		case <-done:
		}
	}()
	go func() {
		defer close(wake)
		defer close(done)
		defer file.Close()
		buffer := make([]byte, 8192)
		for {
			n, err := file.Read(buffer)
			if err != nil {
				if ctx.Err() == nil {
					log.Printf("guest task event source unavailable: %v", err)
				}
				return
			}
			for start := 0; start+unix.SizeofInotifyEvent <= n; {
				header := buffer[start : start+unix.SizeofInotifyEvent]
				mask := binary.NativeEndian.Uint32(header[4:8])
				if mask&(unix.IN_IGNORED|unix.IN_UNMOUNT|unix.IN_MOVE_SELF|unix.IN_DELETE_SELF) != 0 {
					log.Print("guest task event watch lost")
					return
				}
				length := int(binary.NativeEndian.Uint32(header[12:16]))
				nameStart := start + unix.SizeofInotifyEvent
				if length < 0 || length > n-nameStart {
					log.Print("truncated guest task event")
					return
				}
				name := string(bytes.TrimRight(buffer[nameStart:nameStart+length], "\x00"))
				start = nameStart + length
				if name != "active" && name != "index" && mask&unix.IN_Q_OVERFLOW == 0 {
					continue
				}
				select {
				case wake <- struct{}{}:
				default:
				}
			}
		}
	}()
	return wake, nil
}

func validateTaskDirectory(directory string) error {
	if !filepath.IsAbs(directory) {
		return fmt.Errorf("guest task event directory must be an absolute path")
	}
	return nil
}
