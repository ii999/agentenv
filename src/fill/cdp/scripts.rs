//! Fixed, owned scripts evaluated in an isolated world of the target frame.
//! There is no caller-supplied JavaScript anywhere in the backend.
//!
//! These reproduce the definitions Playwright's injected script applies for
//! `fill`: visibility from `computeBox`, disabled state from `getAriaDisabled`,
//! readonly state from `getReadonly`, the fillable input types from `fill`,
//! and text selection from `selectText`. The role computation covers the
//! elements the filling contract accepts (form controls and contenteditable
//! elements) rather than Playwright's complete implicit-role table.

/// `function(selector) -> Element[]`: all matches in the document, piercing
/// open shadow roots in document order. Closed shadow roots are not visible.
pub const QUERY_ALL: &str = r#"function(selector) {
  const matches = [];
  const visit = (root) => {
    for (const element of root.querySelectorAll(selector)) matches.push(element);
    for (const element of root.querySelectorAll('*')) {
      if (element.shadowRoot) visit(element.shadowRoot);
    }
  };
  visit(document);
  return matches;
}"#;

/// `function() -> number` on an array of matches.
pub const COUNT: &str = "function() { return this.length; }";

/// `function() -> Element` on a single-element array.
pub const FIRST: &str = "function() { return this[0]; }";

/// `function(other) -> boolean` on an element.
pub const SAME_NODE: &str = "function(other) { return this === other; }";

/// `function() -> boolean` on an element: still part of its document.
pub const IS_CONNECTED: &str = "function() { return this.isConnected; }";

/// `function() -> boolean` on an element: is it an iframe/frame owner.
pub const IS_FRAME_OWNER: &str = r#"function() {
  const tag = this.nodeName.toUpperCase();
  return tag === 'IFRAME' || tag === 'FRAME';
}"#;

/// `function() -> {tag, inputType, state}` on an element, where `state` is
/// one of `ok`, `unfillable`, `hidden`, `disabled`, `readonly`, evaluated in
/// the same order as the reference client.
pub const CHECK_STATE: &str = r#"function() {
  const element = this;
  const tag = element.nodeName.toLowerCase();
  const inputType = tag === 'input' ? element.type.toLowerCase() : null;
  const result = (state) => ({ tag, inputType, state });

  const kInputTypesToTypeInto = new Set(['', 'email', 'number', 'password', 'search', 'tel', 'text', 'url']);
  if (tag === 'input') {
    if (!kInputTypesToTypeInto.has(inputType)) return result('unfillable');
  } else if (tag !== 'textarea' && !element.isContentEditable) {
    return result('unfillable');
  }

  const isStyleVisible = (node, style) => {
    if (Element.prototype.checkVisibility && !node.checkVisibility()) return false;
    return style.visibility === 'visible';
  };
  const isTextNodeVisible = (node) => {
    const range = node.ownerDocument.createRange();
    range.selectNode(node);
    const rect = range.getBoundingClientRect();
    return rect.width > 0 && rect.height > 0;
  };
  const isVisible = (node) => {
    const style = node.ownerDocument.defaultView.getComputedStyle(node);
    if (!style) return true;
    if (style.display === 'contents') {
      for (let child = node.firstChild; child; child = child.nextSibling) {
        if (child.nodeType === 1 && isVisible(child)) return true;
        if (child.nodeType === 3 && isTextNodeVisible(child)) return true;
      }
      return false;
    }
    if (!isStyleVisible(node, style)) return false;
    const rect = node.getBoundingClientRect();
    return rect.width > 0 && rect.height > 0;
  };
  if (!isVisible(element)) return result('hidden');

  const kAriaDisabledRoles = ['application', 'button', 'composite', 'gridcell', 'group', 'input', 'link', 'menuitem', 'scrollbar', 'separator', 'tab', 'checkbox', 'columnheader', 'combobox', 'grid', 'listbox', 'menu', 'menubar', 'menuitemcheckbox', 'menuitemradio', 'option', 'radio', 'radiogroup', 'row', 'rowheader', 'searchbox', 'select', 'slider', 'spinbutton', 'switch', 'tablist', 'textbox', 'toolbar', 'tree', 'treegrid', 'treeitem'];
  const kAriaReadonlyRoles = ['checkbox', 'combobox', 'grid', 'gridcell', 'listbox', 'radiogroup', 'slider', 'spinbutton', 'textbox', 'columnheader', 'rowheader', 'searchbox', 'switch', 'treegrid'];
  const implicitRole = (node) => {
    const name = node.nodeName.toUpperCase();
    if (name === 'INPUT') {
      const type = node.type.toLowerCase();
      if (type === 'search') return node.hasAttribute('list') ? 'combobox' : 'searchbox';
      if (['email', 'tel', 'text', 'url', ''].includes(type)) return node.hasAttribute('list') ? 'combobox' : 'textbox';
      if (type === 'number') return 'spinbutton';
      if (type === 'hidden' || type === 'file') return null;
      return 'textbox';
    }
    if (name === 'TEXTAREA') return 'textbox';
    return null;
  };
  const role = (node) => {
    const explicit = (node.getAttribute('role') || '').trim().split(/\s+/)[0];
    return explicit || implicitRole(node);
  };
  const parentOrHost = (node) => node.parentElement || (node.parentNode && node.parentNode.nodeType === 11 ? node.parentNode.host : null);
  const ariaDisabledInChain = (node) => {
    if (!node) return false;
    const attribute = node.getAttribute('aria-disabled');
    if (attribute === 'true') return true;
    if (attribute === 'false') return false;
    return ariaDisabledInChain(parentOrHost(node));
  };
  const belongsToDisabledFieldset = (node) => {
    const fieldset = node.closest('FIELDSET[DISABLED]');
    if (!fieldset) return false;
    const legend = fieldset.querySelector(':scope > LEGEND');
    return !legend || !legend.contains(node);
  };
  const nativelyDisabled = (node) => {
    const name = node.nodeName.toUpperCase();
    const isControl = ['BUTTON', 'INPUT', 'SELECT', 'TEXTAREA', 'OPTION', 'OPTGROUP'].includes(name);
    return isControl && (node.hasAttribute('disabled') || belongsToDisabledFieldset(node));
  };
  const explicitAriaDisabled = (node) => kAriaDisabledRoles.includes(role(node) || '') && ariaDisabledInChain(node);
  if (nativelyDisabled(element) || explicitAriaDisabled(element)) return result('disabled');

  const readonly = (node) => {
    const name = node.nodeName.toUpperCase();
    if (['INPUT', 'TEXTAREA', 'SELECT'].includes(name)) return node.hasAttribute('readonly');
    if (kAriaReadonlyRoles.includes(role(node) || '')) return node.getAttribute('aria-readonly') === 'true';
    if (node.isContentEditable) return false;
    return 'error';
  };
  const readonlyState = readonly(element);
  if (readonlyState === 'error') return result('unfillable');
  if (readonlyState) return result('readonly');
  return result('ok');
}"#;

/// `function() -> boolean` on an element: Playwright's `selectText` followed by
/// focus, so the subsequent `Input.insertText` replaces the content.
pub const SELECT_AND_FOCUS: &str = r#"function() {
  const element = this;
  const tag = element.nodeName.toLowerCase();
  if (tag === 'input') {
    element.select();
    element.focus();
  } else if (tag === 'textarea') {
    element.selectionStart = 0;
    element.selectionEnd = element.value.length;
    element.focus();
  } else {
    element.focus();
    const range = element.ownerDocument.createRange();
    range.selectNodeContents(element);
    const selection = element.ownerDocument.defaultView.getSelection();
    selection.removeAllRanges();
    selection.addRange(range);
  }
  return element.getRootNode().activeElement === element && element.ownerDocument.hasFocus();
}"#;
