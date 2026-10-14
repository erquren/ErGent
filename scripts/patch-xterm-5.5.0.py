from pathlib import Path
import json
import sys

root = Path(sys.argv[1])
assert json.loads((root / 'package.json').read_text())['version'] == '5.5.0'
changed = {}
def edit(path,old,new):
 p=root/path;s=changed.get(p, p.read_text());assert s.count(old)==1,(path,old,s.count(old));changed[p]=s.replace(old,new)
edit('src/browser/Viewport.ts','  public syncScrollArea(immediate: boolean = false): void {','''  public syncScrollArea(immediate: boolean = false): void {
    // reset/open can leave callbacks queued after the renderer is disposed.
    if (this._isDisposed) return;''')
edit('src/common/services/Services.ts','export interface ICoreService {','''export interface ICoreService {
  /** Invalidates deferred user input across readiness, reset and disposal. */
  readonly inputGeneration: number;''')
edit('src/common/services/CoreService.ts','  public serviceBrand: any;','  public serviceBrand: any;\n  public inputGeneration = 0;')
edit('src/common/services/CoreService.ts','    this.decPrivateModes = clone(DEFAULT_DEC_PRIVATE_MODES);\n  }\n\n  public reset()', '''    this.decPrivateModes = clone(DEFAULT_DEC_PRIVATE_MODES);
    this.register(this._optionsService.onSpecificOptionChange('disableStdin', () => this.inputGeneration++));
  }

  public override dispose(): void {
    this.inputGeneration++;
    super.dispose();
  }

  public reset()''')
edit('src/common/services/CoreService.ts','  public reset(): void {','  public reset(): void {\n    this.inputGeneration++;')
edit('src/browser/input/CompositionHelper.ts','  private _dataAlreadySent: string;','''  private _dataAlreadySent: string;
  private _compositionGeneration = -1;

  private _canSend(generation: number): boolean {
    return generation >= 0 && generation === this._coreService.inputGeneration && !this._optionsService.rawOptions.disableStdin;
  }''')
edit('src/browser/input/CompositionHelper.ts','  public compositionstart(): void {','''  public compositionstart(): void {
    this._compositionGeneration = this._optionsService.rawOptions.disableStdin ? -1 : this._coreService.inputGeneration;''')
edit('src/browser/input/CompositionHelper.ts','  public compositionupdate(ev: Pick<CompositionEvent, \'data\'>): void {','''  public compositionupdate(ev: Pick<CompositionEvent, 'data'>): void {
    if (!this._canSend(this._compositionGeneration)) return;''')
edit('src/browser/input/CompositionHelper.ts','    if (!waitForPropagation) {','''    const generation = this._compositionGeneration;
    if (!this._canSend(generation)) {
      this._isSendingComposition = false;
      return;
    }
    if (!waitForPropagation) {''')
edit('src/browser/input/CompositionHelper.ts','        if (this._isSendingComposition) {','        if (this._isSendingComposition && this._canSend(generation)) {')
edit('src/browser/input/CompositionHelper.ts','  private _handleAnyTextareaChanges(): void {','''  private _handleAnyTextareaChanges(): void {
    const generation = this._optionsService.rawOptions.disableStdin ? -1 : this._coreService.inputGeneration;''')
edit('src/browser/input/CompositionHelper.ts','      if (!this._isComposing) {','      if (!this._isComposing && this._canSend(generation)) {')
edit('src/browser/input/CompositionHelper.ts','    if (!this._isComposing) {','    if (!this._isComposing || !this._canSend(this._compositionGeneration)) {')
# Match the shipped entry point exactly, failing closed if the pinned source changes.
edit('lib/xterm.js','syncScrollArea(e=!1){','syncScrollArea(e=!1){if(this._isDisposed)return;')
edit('lib/xterm.js','this.isCursorInitialized=!1,this.isCursorHidden=!1,this._onData=', 'this.inputGeneration=0,this.isCursorInitialized=!1,this.isCursorHidden=!1,this._onData=')
edit('lib/xterm.js','this.decPrivateModes=(0,n.clone)(l)}reset(){this.modes=(0,n.clone)(c)', 'this.decPrivateModes=(0,n.clone)(l),this.register(this._optionsService.onSpecificOptionChange("disableStdin",(()=>this.inputGeneration++)))}dispose(){this.inputGeneration++;super.dispose()}reset(){this.inputGeneration++;this.modes=(0,n.clone)(c)')
edit('lib/xterm.js','this._dataAlreadySent=""}compositionstart(){','this._dataAlreadySent="",this._compositionGeneration=-1}_canSend(e){return e>=0&&e===this._coreService.inputGeneration&&!this._optionsService.rawOptions.disableStdin}compositionstart(){this._compositionGeneration=this._optionsService.rawOptions.disableStdin?-1:this._coreService.inputGeneration;')
edit('lib/xterm.js','compositionupdate(e){','compositionupdate(e){if(!this._canSend(this._compositionGeneration))return;')
edit('lib/xterm.js','_finalizeComposition(e){if(this._compositionView.classList.remove("active"),this._isComposing=!1,e){','_finalizeComposition(e){this._compositionView.classList.remove("active");this._isComposing=!1;const generation=this._compositionGeneration;if(!this._canSend(generation)){this._isSendingComposition=!1;return}if(e){')
edit('lib/xterm.js','if(this._isSendingComposition){let t;','if(this._isSendingComposition&&this._canSend(generation)){let t;')
edit('lib/xterm.js','_handleAnyTextareaChanges(){const e=this._textarea.value;setTimeout((()=>{if(!this._isComposing){','_handleAnyTextareaChanges(){const generation=this._optionsService.rawOptions.disableStdin?-1:this._coreService.inputGeneration;const e=this._textarea.value;setTimeout((()=>{if(!this._isComposing&&this._canSend(generation)){')
edit('lib/xterm.js','updateCompositionElements(e){if(this._isComposing){','updateCompositionElements(e){if(this._isComposing&&this._canSend(this._compositionGeneration)){')
# The bundled source map describes the original minified bytes; do not ship a misleading map.
edit('lib/xterm.js','//# sourceMappingURL=xterm.js.map','//# Source map omitted: see the pinned ErGent patch and upstream TypeScript sources.')

# Validate every pinned replacement before writing any package file.
for path, source in changed.items():
    path.write_text(source)
