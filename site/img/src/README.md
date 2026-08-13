# Source for generated images

`og-card.html` renders `../og.png` (the 1200x630 link-preview card). It is not
part of the site — it's excluded from the deploy.

To regenerate after a copy or brand change:

    python3 -m http.server 8901 --directory img/src
    # screenshot http://localhost:8901/og-card.html at exactly 1200x630
    # save to img/og.png

Anything that renders a 1200x630 viewport works; the page is self-contained
(no external fonts, no scripts) so it looks the same everywhere.

The product screenshots in `../` are captured from the apps' own fixture data —
see the note in the repo README for the exact commands. None of them touch a
running charterd.
