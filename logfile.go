package main

import (
	"os"
	"strings"
	"sync"
)

// rotatingLog écrit le journal et le fait tourner quand il dépasse max octets :
// journal.txt devient journal.1.txt (l'historique précédent reste consultable).
type rotatingLog struct {
	mu   sync.Mutex
	path string
	max  int64
	f    *os.File
	size int64
}

func openRotatingLog(path string, max int64) (*rotatingLog, error) {
	l := &rotatingLog{path: path, max: max}
	if err := l.open(); err != nil {
		return nil, err
	}
	return l, nil
}

func (l *rotatingLog) open() error {
	f, err := os.OpenFile(l.path, os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0o644)
	if err != nil {
		return err
	}
	l.f, l.size = f, 0
	if st, err := f.Stat(); err == nil {
		l.size = st.Size()
	}
	return nil
}

func (l *rotatingLog) backupPath() string {
	return strings.TrimSuffix(l.path, ".txt") + ".1.txt"
}

func (l *rotatingLog) Write(p []byte) (int, error) {
	l.mu.Lock()
	defer l.mu.Unlock()
	if l.f != nil && l.size > 0 && l.size+int64(len(p)) > l.max {
		l.f.Close()
		l.f = nil
		os.Remove(l.backupPath())
		os.Rename(l.path, l.backupPath())
	}
	if l.f == nil {
		if err := l.open(); err != nil {
			return 0, err
		}
	}
	n, err := l.f.Write(p)
	l.size += int64(n)
	return n, err
}
