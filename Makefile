PYTHON ?= python3
PYTHONPATH := $(CURDIR)/src

.PHONY: run demo snapshot test smoke verify package clean

run:
	PYTHONPATH=$(PYTHONPATH) $(PYTHON) -m cutting_board

demo:
	PYTHONPATH=$(PYTHONPATH) $(PYTHON) -m cutting_board --demo

snapshot:
	PYTHONPATH=$(PYTHONPATH) $(PYTHON) -m cutting_board --snapshot

test:
	PYTHONPATH=$(PYTHONPATH) $(PYTHON) -m unittest discover -s tests -v

smoke:
	xvfb-run -a env PYTHONPATH=$(PYTHONPATH) $(PYTHON) -m cutting_board --demo --auto-close 1

verify:
	./scripts/verify.sh

package:
	./scripts/build-deb.sh

clean:
	rm -rf build *.egg-info src/*.egg-info
	find . -type d -name __pycache__ -prune -exec rm -rf {} +
