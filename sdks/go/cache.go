package flagship

import (
	"container/list"
	"sync"
	"time"
)

type responseCache struct {
	mu      sync.Mutex
	ttl     time.Duration
	maxSize int
	items   map[string]*list.Element
	order   *list.List
}

type responseCacheEntry struct {
	key       string
	response  EvaluationResponse
	expiresAt time.Time
}

func newResponseCache(ttl time.Duration, maxSize int) *responseCache {
	if maxSize <= 0 {
		maxSize = defaultCacheSize
	}
	return &responseCache{
		ttl:     ttl,
		maxSize: maxSize,
		items:   make(map[string]*list.Element),
		order:   list.New(),
	}
}

func (c *responseCache) get(key string) (EvaluationResponse, bool) {
	c.mu.Lock()
	defer c.mu.Unlock()

	element, ok := c.items[key]
	if !ok {
		return EvaluationResponse{}, false
	}

	entry := element.Value.(*responseCacheEntry)
	if !entry.expiresAt.After(time.Now()) {
		c.removeElement(element)
		return EvaluationResponse{}, false
	}

	c.order.MoveToFront(element)
	return entry.response, true
}

func (c *responseCache) set(key string, response EvaluationResponse) {
	c.mu.Lock()
	defer c.mu.Unlock()

	if element, ok := c.items[key]; ok {
		entry := element.Value.(*responseCacheEntry)
		entry.response = response
		entry.expiresAt = time.Now().Add(c.ttl)
		c.order.MoveToFront(element)
		return
	}

	entry := &responseCacheEntry{key: key, response: response, expiresAt: time.Now().Add(c.ttl)}
	element := c.order.PushFront(entry)
	c.items[key] = element

	for len(c.items) > c.maxSize {
		oldest := c.order.Back()
		if oldest == nil {
			return
		}
		c.removeElement(oldest)
	}
}

func (c *responseCache) clear() {
	c.mu.Lock()
	defer c.mu.Unlock()

	c.items = make(map[string]*list.Element)
	c.order.Init()
}

func (c *responseCache) removeElement(element *list.Element) {
	c.order.Remove(element)
	entry := element.Value.(*responseCacheEntry)
	delete(c.items, entry.key)
}
