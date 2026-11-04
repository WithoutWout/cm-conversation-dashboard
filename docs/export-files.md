# Export files

What counts as an older version of an export, and how the app offers to delete it.

---

## The offer to delete older exports

CM.com names each export with its date, so a new drop sits beside the old one
instead of replacing it. The loader only reads the newest, and the rest pile up.

**One trigger: a load.** `get_data` lists the candidates in
`dataSource.oldExports` and `loadData` hands them to `offerOldExports`. That one
path covers launch, choosing a folder, Refresh, and a new export, because a new
export arrives as "Export Files Updated" and the reload it asks for is a load.
It is deliberately not offered from the watcher event: until that reload the app
is still showing the file that would be called old.

**What counts as older (`older_versions`).** The sort is the loader's own, so
"kept" and "loaded" cannot be two different files. A less recent file is only a
candidate when both of these hold, because this ends in a delete:

- **Same text before the pattern.** `..._nl_ArticlesExport_...` and
  `..._de_ArticlesExport_...` are two exports, not two versions of one.
- **Its name does not carry a later date than the kept file.** A download or a
  sync can give an old file a fresh modification time. The loader then reads the
  old one, which is a problem of its own, but the actually newer export must not
  be what gets offered for deletion.

**A kind whose newest file loaded nothing offers nothing.** A truncated or
corrupt export parses as empty, and at that moment the previous version is the
only good copy left.

**Deleting takes names, never paths (`remove_older_exports`).** The folder is the
watched one, and each name is looked up among the older versions read at that
moment. Nothing else can be reached, and a file that has become the newest since
the offer was shown is refused.

**"Delete" means the Trash or Recycle Bin**, through the `trash` crate. On macOS
it uses `NSFileManager`: the crate's default goes through Finder, which asks for
permission to control Finder. A folder on a drive with no Trash reports a failure
on macOS and the file stays listed. On Windows the shell decides, and it may
delete a file on a network share outright.

**The app's own deletion is not news.** The watcher reports removals, so
accepting the offer would be answered by "Export Files Updated".
`WatchState.own_removals` holds each name for `OWN_REMOVAL_WINDOW`, and an event
about only those names is dropped. An event naming any other export file still
reports. The `#[ignore]`d `a_real_move_to_trash_reports_only_the_file_it_moved`
runs one file through the real watcher and Trash; it leaves that file in the
user's Trash, which is why it is ignored.

**"Not now"** lasts until the app restarts, or until another file becomes old, so
Refresh does not ask again but the next launch does. It is held in memory on
purpose: the request was to be asked at launch.

**Never on top of another dialog.** Like `checkMissingDays`, it waits for the
screen to be free, since the update prompt opens at launch too. A later load
replaces the wait, and a load under an open offer refreshes its list.
