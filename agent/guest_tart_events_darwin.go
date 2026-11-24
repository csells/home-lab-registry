//go:build darwin

package main

import (
	"context"
	"errors"
	"log"
	"os"
	"path/filepath"

	"golang.org/x/sys/unix"
)

// Bindings on the Tart inventory directories and the processes holding Tart's
// config lock invalidate the cached report. Only tart list reports guest state.
func tartGuestEvents(ctx context.Context, home string) (<-chan struct{}, error) {
	if home == "" {
		home = os.Getenv("TART_HOME")
	}
	if home == "" {
		userHome, err := os.UserHomeDir()
		if err != nil {
			return nil, err
		}
		home = filepath.Join(userHome, ".tart")
	}
	root := filepath.Join(home, "vms")
	queue, err := unix.Kqueue()
	if err != nil {
		return nil, err
	}
	unix.CloseOnExec(queue)
	watches := tartWatches{queue: queue, directories: map[string]int{}, processes: map[int32]bool{}}
	if err := watches.addDirectory(root); err != nil {
		unix.Close(queue)
		return nil, err
	}
	wake := make(chan struct{}, 1)
	go watches.run(ctx, root, wake)
	return wake, nil
}

type tartWatches struct {
	queue       int
	directories map[string]int
	processes   map[int32]bool
}

func (w *tartWatches) addDirectory(path string) error {
	if _, found := w.directories[path]; found {
		return nil
	}
	fd, err := unix.Open(path, unix.O_EVTONLY|unix.O_CLOEXEC|unix.O_DIRECTORY|unix.O_NOFOLLOW, 0)
	if err != nil {
		return err
	}
	change := unix.Kevent_t{Ident: uint64(fd), Filter: unix.EVFILT_VNODE, Flags: unix.EV_ADD | unix.EV_CLEAR,
		Fflags: unix.NOTE_WRITE | unix.NOTE_DELETE | unix.NOTE_RENAME | unix.NOTE_REVOKE}
	if _, err := unix.Kevent(w.queue, []unix.Kevent_t{change}, nil, nil); err != nil {
		unix.Close(fd)
		return err
	}
	w.directories[path] = fd
	return nil
}

func tartLockPID(path string) int32 {
	fd, err := unix.Open(filepath.Join(path, "config.json"), unix.O_RDONLY|unix.O_CLOEXEC|unix.O_NOFOLLOW, 0)
	if err != nil {
		return 0
	}
	defer unix.Close(fd)
	lock := unix.Flock_t{Type: unix.F_RDLCK, Whence: unix.SEEK_SET}
	if err := unix.FcntlFlock(uintptr(fd), unix.F_GETLK, &lock); err != nil || lock.Type != unix.F_WRLCK {
		return 0
	}
	return lock.Pid
}

func (w *tartWatches) refresh(root string) error {
	entries, err := os.ReadDir(root)
	if err != nil {
		return err
	}
	current := map[string]bool{root: true}
	for _, entry := range entries {
		if !entry.IsDir() {
			continue
		}
		path := filepath.Join(root, entry.Name())
		current[path] = true
		if err := w.addDirectory(path); err != nil && !os.IsNotExist(err) {
			log.Printf("Tart guest directory watch unavailable: %v", err)
		}
		pid := tartLockPID(path)
		if pid <= 0 || w.processes[pid] {
			continue
		}
		change := unix.Kevent_t{Ident: uint64(pid), Filter: unix.EVFILT_PROC,
			Flags: unix.EV_ADD | unix.EV_ONESHOT, Fflags: unix.NOTE_EXIT}
		if _, err := unix.Kevent(w.queue, []unix.Kevent_t{change}, nil, nil); err == nil {
			w.processes[pid] = true
		} else if !errors.Is(err, unix.ESRCH) {
			log.Printf("Tart guest exit watch unavailable: %v", err)
		}
	}
	for path, fd := range w.directories {
		if !current[path] {
			unix.Close(fd)
			delete(w.directories, path)
		}
	}
	return nil
}

func (w *tartWatches) run(ctx context.Context, root string, wake chan<- struct{}) {
	defer close(wake)
	defer unix.Close(w.queue)
	defer func() {
		for _, fd := range w.directories {
			unix.Close(fd)
		}
	}()
	events := make([]unix.Kevent_t, 32)
	timeout := unix.Timespec{Sec: 5}
	for ctx.Err() == nil {
		if err := w.refresh(root); err != nil {
			log.Printf("Tart inventory event source unavailable: %v", err)
			return
		}
		for ctx.Err() == nil {
			count, err := unix.Kevent(w.queue, nil, events, &timeout)
			if errors.Is(err, unix.EINTR) || (err == nil && count == 0) {
				continue
			}
			if err != nil {
				log.Printf("Tart inventory event source unavailable: %v", err)
				return
			}
			for _, event := range events[:count] {
				if event.Filter == unix.EVFILT_PROC {
					delete(w.processes, int32(event.Ident))
				}
				if event.Filter == unix.EVFILT_VNODE && event.Fflags&(unix.NOTE_DELETE|unix.NOTE_RENAME|unix.NOTE_REVOKE) != 0 {
					for path, fd := range w.directories {
						if event.Ident == uint64(fd) {
							unix.Close(fd)
							delete(w.directories, path)
							break
						}
					}
				}
			}
			if err := w.addDirectory(root); err != nil {
				log.Printf("Tart inventory event source unavailable: %v", err)
				return
			}
			select {
			case wake <- struct{}{}:
			default:
			}
			break
		}
	}
}
