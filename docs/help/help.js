// pgtc help — the side menu, previous / next links and picture zoom of every page.
// Plain script, no libraries: the help works from the file system, offline.
(function () {
  // The chapters, in reading order: file name and title.
  var PAGES = [
    ['index.html', 'Welcome'],
    ['start.html', 'Getting started'],
    ['window.html', 'The window, menus and keys'],
    ['connections.html', 'Connections'],
    ['tree.html', 'Object tree'],
    ['objects.html', 'Object view (definitions)'],
    ['query.html', 'Query tabs'],
    ['editor.html', 'SQL editor and autocomplete'],
    ['files.html', 'SQL files and result export'],
    ['psql.html', 'psql-style and generate commands'],
    ['templates.html', 'Developer templates'],
    ['template-editor.html', 'Developer template editor'],
    ['ai.html', 'AI assistant'],
    ['pgdef.html', 'Database definition: export and load'],
    ['dashboard.html', 'Server dashboard'],
    ['profiler.html', 'SQL profiler'],
    ['dbstats.html', 'Database statistics'],
    ['settings.html', 'Settings, colours and keys'],
    ['reference.html', 'Reference'],
    ['trouble.html', 'Troubleshooting']
  ];

  var here = location.pathname.split('/').pop() || 'index.html';
  var at = 0;
  PAGES.forEach(function (p, i) { if (p[0] === here) { at = i; } });

  var main = document.querySelector('main');
  if (!main) { return; }

  // layout: side menu + main
  var layout = document.createElement('div');
  layout.id = 'layout';
  var nav = document.createElement('nav');
  nav.id = 'side';
  main.parentNode.insertBefore(layout, main);
  layout.appendChild(nav);
  layout.appendChild(main);

  var html = '<p class="brand"><a href="index.html">pgtc Help</a></p>' +
    '<p class="sub">Postgres terminal commander</p><ol>';
  PAGES.forEach(function (p, i) {
    html += '<li' + (i === at ? ' class="here"' : '') + '><a href="' + p[0] + '">' + p[1] + '</a>';
    if (i === at) {
      // the sections of this page
      var hs = main.querySelectorAll('h2[id]');
      if (hs.length) {
        html += '<ol class="sections">';
        Array.prototype.forEach.call(hs, function (h) {
          html += '<li><a href="#' + h.id + '">' + h.textContent + '</a></li>';
        });
        html += '</ol>';
      }
    }
    html += '</li>';
  });
  nav.innerHTML = html + '</ol>';

  // previous / next
  var pager = document.createElement('div');
  pager.className = 'pager';
  var prev = PAGES[at - 1], next = PAGES[at + 1];
  pager.innerHTML =
    (prev ? '<a href="' + prev[0] + '">&larr; ' + prev[1] + '</a>' : '<span></span>') +
    (next ? '<a href="' + next[0] + '">' + next[1] + ' &rarr;</a>' : '<span></span>');
  main.appendChild(pager);
  var foot = document.createElement('footer');
  foot.textContent = 'pgtc — Postgres terminal commander. Screenshots: drawn by pgtc\'s own ' +
    'renderer with sample data in the shape of the AdventureWorks database.';
  main.appendChild(foot);

  // click a screenshot to see it in full size
  var zoom = document.createElement('div');
  zoom.id = 'zoom';
  zoom.innerHTML = '<img alt="">';
  document.body.appendChild(zoom);
  main.addEventListener('click', function (e) {
    var t = e.target;
    if (t.tagName === 'IMG' && t.closest('figure')) {
      zoom.firstChild.src = t.src;
      zoom.classList.add('on');
    }
  });
  zoom.addEventListener('click', function () { zoom.classList.remove('on'); });
  document.addEventListener('keydown', function (e) {
    if (e.key === 'Escape') { zoom.classList.remove('on'); }
  });
})();
