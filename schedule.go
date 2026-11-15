package main

import (
	"errors"
	"fmt"
	"strconv"
	"strings"
	"time"
)

// AlbumRule : albums à utiliser à la place des albums habituels pendant une saison
// et/ou une plage horaire. Sans saison ni horaires, la règle s'applique tout le temps.
type AlbumRule struct {
	Albums []string `json:"albums"`
	Season string   `json:"season"` // "" | spring | summer | autumn | winter
	From   string   `json:"from"`   // "HH:MM" ; vide avec To = toute la journée
	To     string   `json:"to"`
}

// parseClock lit « HH:MM » et renvoie le nombre de minutes depuis minuit.
func parseClock(s string) (int, bool) {
	h, m, ok := strings.Cut(strings.TrimSpace(s), ":")
	if !ok {
		return 0, false
	}
	hh, err1 := strconv.Atoi(h)
	mm, err2 := strconv.Atoi(m)
	if err1 != nil || err2 != nil || hh < 0 || hh > 23 || mm < 0 || mm > 59 {
		return 0, false
	}
	return hh*60 + mm, true
}

// normalizeClock accepte une heure vide ou « H:MM » et la renvoie sous la forme « HH:MM ».
func normalizeClock(s string) (string, error) {
	if strings.TrimSpace(s) == "" {
		return "", nil
	}
	v, ok := parseClock(s)
	if !ok {
		return "", fmt.Errorf("heure %q invalide (format attendu : HH:MM)", s)
	}
	return fmt.Sprintf("%02d:%02d", v/60, v%60), nil
}

// inClockRange : l'heure t est dans la plage [from, to). Une plage dont la fin précède le
// début passe par minuit (22:00 → 07:00). Plage vide, incomplète ou de durée nulle : toujours vrai.
func inClockRange(from, to string, t time.Time) bool {
	f, ok1 := parseClock(from)
	e, ok2 := parseClock(to)
	if !ok1 || !ok2 || f == e {
		return true
	}
	now := t.Hour()*60 + t.Minute()
	if f < e {
		return now >= f && now < e
	}
	return now >= f || now < e
}

// seasonOf : saison météorologique (hémisphère nord) du mois donné.
func seasonOf(m time.Month) string {
	switch m {
	case time.March, time.April, time.May:
		return "spring"
	case time.June, time.July, time.August:
		return "summer"
	case time.September, time.October, time.November:
		return "autumn"
	}
	return "winter"
}

func (r AlbumRule) matches(t time.Time) bool {
	return (r.Season == "" || r.Season == seasonOf(t.Month())) && inClockRange(r.From, r.To, t)
}

// activeAt : les changements automatiques sont autorisés à cette heure.
func (c *Config) activeAt(t time.Time) bool { return inClockRange(c.ActiveFrom, c.ActiveTo, t) }

func uniqueIDs(lists ...[]string) []string {
	seen := map[string]bool{}
	var out []string
	for _, l := range lists {
		for _, id := range l {
			if !seen[id] {
				seen[id] = true
				out = append(out, id)
			}
		}
	}
	return out
}

// albumsAt renvoie les albums à utiliser à l'instant t : ceux des règles qui s'appliquent,
// sinon les albums habituels.
func (c *Config) albumsAt(t time.Time) []string {
	var lists [][]string
	for _, r := range c.AlbumRules {
		if r.matches(t) {
			lists = append(lists, r.Albums)
		}
	}
	if ids := uniqueIDs(lists...); len(ids) > 0 {
		return ids
	}
	return c.Albums
}

// allAlbums : tous les albums référencés (habituels et règles).
func (c *Config) allAlbums() []string {
	lists := [][]string{c.Albums}
	for _, r := range c.AlbumRules {
		lists = append(lists, r.Albums)
	}
	return uniqueIDs(lists...)
}

// normalizeSchedule valide la plage horaire et les règles d'albums.
func (c *Config) normalizeSchedule() error {
	var err error
	if c.ActiveFrom, err = normalizeClock(c.ActiveFrom); err != nil {
		return err
	}
	if c.ActiveTo, err = normalizeClock(c.ActiveTo); err != nil {
		return err
	}
	if (c.ActiveFrom == "") != (c.ActiveTo == "") {
		return errors.New("plage horaire incomplète : indique l'heure de début et l'heure de fin, ou aucune")
	}
	var rules []AlbumRule
	for _, r := range c.AlbumRules {
		out := AlbumRule{Season: strings.ToLower(strings.TrimSpace(r.Season))}
		switch out.Season {
		case "", "spring", "summer", "autumn", "winter":
		default:
			return fmt.Errorf("saison %q inconnue (spring, summer, autumn ou winter)", r.Season)
		}
		if out.From, err = normalizeClock(r.From); err != nil {
			return err
		}
		if out.To, err = normalizeClock(r.To); err != nil {
			return err
		}
		if (out.From == "") != (out.To == "") {
			return errors.New("règle d'albums : plage horaire incomplète")
		}
		for _, a := range r.Albums {
			id, err := parseAlbumID(a)
			if err != nil {
				return err
			}
			out.Albums = append(out.Albums, id)
		}
		out.Albums = uniqueIDs(out.Albums)
		if len(out.Albums) == 0 {
			continue // règle sans album : ignorée
		}
		rules = append(rules, out)
	}
	c.AlbumRules = rules
	return nil
}
