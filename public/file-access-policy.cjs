const path = require('path');

function isPathInsideRoot(filePath, root) {
  const relative = path.relative(root, filePath);
  return relative === '' || (!relative.startsWith('..') && !path.isAbsolute(relative));
}

function isPathGranted(filePath, grantedRoots) {
  return [...grantedRoots].some((root) => isPathInsideRoot(filePath, root));
}

function folderGrantPrompt(folder) {
  return {
    type: 'warning',
    title: 'Allow folder access?',
    message: 'This Director movie may load other files from the same folder.',
    detail: `Allow DirPlayer to read files in this folder for the current app session?\n\n${folder}`,
    buttons: ['Allow Folder', 'Cancel'],
    defaultId: 1,
    cancelId: 1,
    noLink: true,
  };
}

module.exports = {
  folderGrantPrompt,
  isPathGranted,
  isPathInsideRoot,
};
