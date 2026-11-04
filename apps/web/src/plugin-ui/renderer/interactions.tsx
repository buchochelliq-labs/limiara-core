import React, {useCallback, useEffect, useMemo, useRef, useState, type FormEvent, type ReactNode} from 'react';
import {Alert, Button} from '../../components/ui';
import type {
  Binding,
  CallerSafeFieldDescriptor,
  CallerSafeResourceDescriptor,
  ConditionExpression,
  PageDefinition,
  ResolvedPage,
  UiNode,
} from '../contracts';

export type MetadataFormMode = 'create' | 'edit' | 'view' | 'quick-create' | 'wizard' | 'embedded' | 'related-record';
export type MetadataValues = Record<string, unknown>;
export type MetadataFieldErrors = Record<string, string>;

export type MetadataActionRequest = {
  action: NonNullable<PageDefinition['actions']>[number];
  values: MetadataValues;
  mode: MetadataFormMode;
  recordId?: string;
  etag?: string;
};

export type MetadataActionResult =
  | {status: 'success'; values?: MetadataValues; etag?: string}
  | {status: 'validation'; fieldErrors?: MetadataFieldErrors; formError?: string}
  | {status: 'failure'; category: 'denied' | 'network' | 'duplicate' | 'unavailable'; message: string; values?: MetadataValues}
  | {status: 'conflict'; latestValues: MetadataValues; latestEtag: string};

export type MetadataInteractionAdapter = {
  execute: (request: MetadataActionRequest) => Promise<MetadataActionResult>;
  navigate?: (target: string, parameters: MetadataValues) => void;
  openOverlay?: (target: string, parameters: MetadataValues) => void;
};

export type ConditionContext = {
  fields: MetadataValues;
  context?: MetadataValues;
  dataSources?: Record<string, unknown>;
  permissions?: ReadonlySet<string>;
};

export type EvaluatedNodeState = {visible: boolean; enabled: boolean; readOnly: boolean; required: boolean};

const blockedPathSegments = new Set(['__proto__', 'constructor', 'prototype']);

function readPath(value: unknown, path: string): unknown {
  if (!path) return value;
  let current = value;
  for (const segment of path.split('.')) {
    if (blockedPathSegments.has(segment) || typeof current !== 'object' || current === null) return undefined;
    current = (current as Record<string, unknown>)[segment];
  }
  return current;
}

export function resolveBinding(binding: Binding, context: ConditionContext): unknown {
  if (binding.source === 'field') return context.fields[binding.field];
  if (binding.source === 'context') return readPath(context.context, binding.path);
  if (binding.source === 'literal') return binding.value;
  return readPath(context.dataSources?.[binding.data_source], binding.path);
}

export function buildActionParameters(action: NonNullable<PageDefinition['actions']>[number], context: ConditionContext, parameterOverrides: MetadataValues = {}) {
  const declared = action.parameters ?? {};
  const parameters = Object.fromEntries(Object.entries(declared).map(([key, binding]) => [key, resolveBinding(binding, context)]));
  for (const [key, value] of Object.entries(parameterOverrides)) {
    if (declared[key]?.source === 'literal') parameters[key] = value;
  }
  return parameters;
}

function sameValue(left: unknown, right: unknown) {
  if (Object.is(left, right)) return true;
  if (left === null || right === null || typeof left !== 'object' || typeof right !== 'object') return false;
  try {
    return JSON.stringify(left) === JSON.stringify(right);
  } catch {
    return false;
  }
}

export function evaluateCondition(condition: ConditionExpression, context: ConditionContext, depth = 0): boolean {
  if (depth > 64) return false;
  if (condition.op === 'and') return condition.args.length > 0 && condition.args.every(item => evaluateCondition(item, context, depth + 1));
  if (condition.op === 'or') return condition.args.length > 0 && condition.args.some(item => evaluateCondition(item, context, depth + 1));
  if (condition.op === 'not') return !evaluateCondition(condition.arg, context, depth + 1);
  if (condition.op === 'equal') return sameValue(resolveBinding(condition.left, context), resolveBinding(condition.right, context));
  if (condition.op === 'not-equal') return !sameValue(resolveBinding(condition.left, context), resolveBinding(condition.right, context));
  if (condition.op === 'exists') {
    const value = resolveBinding(condition.value, context);
    return value !== undefined && value !== null;
  }
  if (condition.op === 'in') {
    const value = resolveBinding(condition.value, context);
    return condition.set.some(candidate => sameValue(value, resolveBinding(candidate, context)));
  }
  return 'permission' in condition && (context.permissions?.has(condition.permission) ?? false);
}

export function evaluateNodeState(node: UiNode, context: ConditionContext, viewMode = false): EvaluatedNodeState {
  const conditions = node.conditions ?? {};
  return {
    visible: conditions.visible ? evaluateCondition(conditions.visible, context) : true,
    enabled: conditions.enabled ? evaluateCondition(conditions.enabled, context) : true,
    readOnly: viewMode || (conditions['read-only'] ? evaluateCondition(conditions['read-only'], context) : false),
    required: conditions.required ? evaluateCondition(conditions.required, context) : false,
  };
}

function fieldName(node: UiNode) {
  const bound = Object.values(node.bindings ?? {}).find(binding => binding.source === 'field');
  if (bound?.source === 'field') return bound.field;
  const field = node.props?.field ?? node.props?.name;
  return typeof field === 'string' ? field : undefined;
}

function walk(root: UiNode, visit: (node: UiNode) => void) {
  visit(root);
  root.children?.forEach(child => walk(child, visit));
}

export function buildSubmissionPayload(page: ResolvedPage, resource: CallerSafeResourceDescriptor | undefined, values: MetadataValues, context: ConditionContext) {
  const hidden = new Set<string>();
  walk(page.root, node => {
    const name = fieldName(node);
    if (name && !evaluateNodeState(node, context).visible) hidden.add(name);
  });
  return Object.fromEntries((resource?.fields ?? [])
    .filter(field => field.writable && (!hidden.has(field.api_name) || field.submit_when_hidden))
    .filter(field => Object.hasOwn(values, field.api_name))
    .map(field => [field.api_name, values[field.api_name]]));
}

type RuntimeOptions = {
  page: ResolvedPage;
  resource?: CallerSafeResourceDescriptor;
  mode: MetadataFormMode;
  initialValues: MetadataValues;
  contextValues: MetadataValues;
  dataSources: Record<string, unknown>;
  permissions: ReadonlySet<string>;
  adapter?: MetadataInteractionAdapter;
  recordId?: string;
  etag?: string;
};

export type MetadataInteractionRuntime = {
  mode: MetadataFormMode;
  values: MetadataValues;
  fieldErrors: MetadataFieldErrors;
  formError: string;
  dirty: boolean;
  submitting: boolean;
  conflict: boolean;
  step: number;
  formRef: React.RefObject<HTMLFormElement | null>;
  field: (node: UiNode) => CallerSafeFieldDescriptor | undefined;
  state: (node: UiNode) => EvaluatedNodeState;
  value: (node: UiNode) => unknown;
  update: (node: UiNode, value: unknown) => void;
  resolve: (binding: Binding) => unknown;
  invoke: (actionId: string, parameterOverrides?: MetadataValues) => Promise<void>;
  /** Navigates to a host target, such as a linked record, through the page adapter. */
  open: (target: string, parameters: MetadataValues) => void;
  submitDefault: () => Promise<void>;
  cancel: (target?: string) => void;
  reviewLatest: () => void;
  keepMyChanges: () => void;
  setStep: (step: number) => void;
};

function initialFormValues(resource: CallerSafeResourceDescriptor | undefined, supplied: MetadataValues) {
  const defaults = Object.fromEntries((resource?.fields ?? []).filter(field => field.default_value !== undefined).map(field => [field.api_name, field.default_value]));
  return {...defaults, ...supplied};
}

function useMetadataInteraction(options: RuntimeOptions): MetadataInteractionRuntime {
  const [values, setValues] = useState<MetadataValues>(() => initialFormValues(options.resource, options.initialValues));
  const [fieldErrors, setFieldErrors] = useState<MetadataFieldErrors>({});
  const [formError, setFormError] = useState('');
  const [dirty, setDirty] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [conflictState, setConflictState] = useState<{latestValues: MetadataValues; latestEtag: string} | null>(null);
  const [etag, setEtag] = useState(options.etag);
  const [step, setStep] = useState(0);
  const [lastAction, setLastAction] = useState<string>();
  const formRef = useRef<HTMLFormElement>(null);
  const fieldMap = useMemo(() => new Map((options.resource?.fields ?? []).map(field => [field.api_name, field])), [options.resource]);
  const conditionContext = useMemo<ConditionContext>(() => ({fields: values, context: options.contextValues, dataSources: options.dataSources, permissions: options.permissions}), [options.contextValues, options.dataSources, options.permissions, values]);
  const viewMode = options.mode === 'view';

  useEffect(() => {
    if (!dirty || typeof window === 'undefined') return;
    const warn = (event: BeforeUnloadEvent) => event.preventDefault();
    window.addEventListener('beforeunload', warn);
    return () => window.removeEventListener('beforeunload', warn);
  }, [dirty]);

  const state = useCallback((node: UiNode) => evaluateNodeState(node, conditionContext, viewMode), [conditionContext, viewMode]);
  const field = useCallback((node: UiNode) => {
    const name = fieldName(node);
    return name ? fieldMap.get(name) : undefined;
  }, [fieldMap]);
  const value = useCallback((node: UiNode) => {
    const name = fieldName(node);
    return name ? values[name] : undefined;
  }, [values]);
  const update = useCallback((node: UiNode, next: unknown) => {
    const name = fieldName(node);
    if (!name || viewMode || !fieldMap.get(name)?.writable) return;
    setValues(current => ({...current, [name]: next}));
    setFieldErrors(current => {
      if (!current[name]) return current;
      const copy = {...current};
      delete copy[name];
      return copy;
    });
    setFormError('');
    setDirty(true);
  }, [fieldMap, viewMode]);

  const focusFirstError = useCallback((errors: MetadataFieldErrors) => {
    const first = Object.keys(errors)[0];
    if (!first) return;
    requestAnimationFrame(() => {
      const controls = formRef.current?.querySelectorAll<HTMLInputElement | HTMLSelectElement | HTMLTextAreaElement>('[name]');
      Array.from(controls ?? []).find(control => control.name === first)?.focus();
    });
  }, []);

  const payload = useCallback(() => {
    return buildSubmissionPayload(options.page, options.resource, values, conditionContext);
  }, [conditionContext, options.page, options.resource, values]);

  const validate = useCallback(() => {
    const errors: MetadataFieldErrors = {};
    walk(options.page.root, node => {
      const descriptor = field(node);
      if (!descriptor) return;
      const nodeState = state(node);
      if (!nodeState.visible) return;
      const current = values[descriptor.api_name];
      if ((descriptor.required || nodeState.required) && (current === undefined || current === null || current === '')) errors[descriptor.api_name] = `${descriptor.label} is required.`;
    });
    setFieldErrors(errors);
    focusFirstError(errors);
    return Object.keys(errors).length === 0;
  }, [field, focusFirstError, options.page.root, state, values]);

  const resolve = useCallback((binding: Binding) => resolveBinding(binding, conditionContext), [conditionContext]);
  const invoke = useCallback(async (actionId: string, parameterOverrides: MetadataValues = {}) => {
    const action = options.page.actions.find(candidate => candidate.id === actionId);
    if (!action) {
      setFormError('This action is no longer available. Refresh the page and try again.');
      return;
    }
    if (action.target === 'form.next') { setStep(current => current + 1); return; }
    if (action.target === 'form.previous') { setStep(current => Math.max(0, current - 1)); return; }
    if (action.target === 'form.cancel') { cancel(); return; }
    const parameters = buildActionParameters(action, conditionContext, parameterOverrides);
    if (action.kind === 'navigate') { options.adapter?.navigate?.(action.target, parameters); return; }
    if (action.kind === 'overlay') { options.adapter?.openOverlay?.(action.target, parameters); return; }
    if (!validate() || !options.adapter) return;
    setSubmitting(true);
    setFormError('');
    setLastAction(actionId);
    try {
      const result = await options.adapter.execute({action, values: payload(), mode: options.mode, recordId: options.recordId, etag});
      if (result.status === 'success') {
        if (result.values) setValues(result.values);
        if (result.etag) setEtag(result.etag);
        setDirty(false);
        setConflictState(null);
        setFieldErrors({});
      } else if (result.status === 'validation') {
        const errors = result.fieldErrors ?? {};
        setFieldErrors(errors);
        setFormError(result.formError ?? 'Check the highlighted fields and try again.');
        focusFirstError(errors);
      } else if (result.status === 'conflict') {
        setConflictState({latestValues: result.latestValues, latestEtag: result.latestEtag});
      } else {
        if (result.values) setValues(result.values);
        setFormError(result.message);
      }
    } catch {
      setFormError('The request could not be completed. Check your connection and try again.');
    } finally {
      setSubmitting(false);
    }
  }, [conditionContext, etag, focusFirstError, options, payload, validate]);

  const submitDefault = useCallback(async () => {
    const expectedOperation = options.mode === 'create' || options.mode === 'quick-create' ? 'create' : options.mode === 'edit' ? 'update' : undefined;
    const operationIds = new Set((options.resource?.operations ?? []).filter(operation => !expectedOperation || operation.kind === expectedOperation).map(operation => operation.id));
    const resourceActions = options.page.actions.filter(candidate => candidate.kind === 'resource-operation');
    const action = options.page.actions.find(candidate => candidate.kind === 'submit')
      ?? resourceActions.find(candidate => operationIds.has(candidate.target.split('.').at(-1) ?? ''))
      ?? (operationIds.size === 0 ? resourceActions[0] : undefined);
    if (action) await invoke(action.id);
  }, [invoke, options.mode, options.page.actions, options.resource?.operations]);
  const cancel = useCallback((target?: string) => {
    if (dirty && typeof window !== 'undefined' && !window.confirm('Discard your unsaved changes?')) return;
    options.adapter?.navigate?.(target ?? 'back', {});
  }, [dirty, options.adapter]);
  const reviewLatest = useCallback(() => {
    if (!conflictState) return;
    setValues(conflictState.latestValues);
    setEtag(conflictState.latestEtag);
    setConflictState(null);
    setDirty(false);
    setFormError('The latest saved version is shown. Review it before saving again.');
  }, [conflictState]);
  const keepMyChanges = useCallback(() => {
    if (!conflictState) return;
    setEtag(conflictState.latestEtag);
    setConflictState(null);
    setFormError('Your changes are preserved and can be saved over the latest version.');
  }, [conflictState]);

  const open = useCallback((target: string, parameters: MetadataValues) => options.adapter?.navigate?.(target, parameters), [options.adapter]);
  return {mode: options.mode, values, fieldErrors, formError, dirty, submitting, conflict: Boolean(conflictState), step, formRef, field, state, value, update, resolve, invoke, open, submitDefault, cancel, reviewLatest, keepMyChanges, setStep};
}

export function MetadataInteractionBoundary({children, ...options}: RuntimeOptions & {children: (runtime: MetadataInteractionRuntime) => ReactNode}) {
  return children(useMetadataInteraction(options));
}

export function MetadataFormShell({runtime, children, label}: {runtime: MetadataInteractionRuntime; children: ReactNode; label: string}) {
  const submit = (event: FormEvent) => {
    event.preventDefault();
    void runtime.submitDefault();
  };
  return <form aria-label={label} className={`meta-form meta-form--${runtime.mode}`} noValidate onSubmit={submit} ref={runtime.formRef}>
    {runtime.conflict && <Alert title="This record changed elsewhere" variant="warning">Your changes are still here.<div className="meta-form__recovery"><Button onClick={runtime.reviewLatest} size="small" type="button" variant="secondary">Review latest</Button><Button onClick={runtime.keepMyChanges} size="small" type="button">Keep my changes</Button></div></Alert>}
    {runtime.formError && <Alert title="The form was not saved" variant="danger">{runtime.formError}</Alert>}
    {children}
    <p aria-live="polite" className="sr-only">{runtime.submitting ? 'Saving.' : runtime.dirty ? 'You have unsaved changes.' : 'No unsaved changes.'}</p>
  </form>;
}
