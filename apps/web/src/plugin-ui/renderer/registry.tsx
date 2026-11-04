import type {ReactNode} from 'react';
import type {CallerSafeResourceDescriptor, HostProfile, UiNode} from '../contracts';
import type {MetadataInteractionRuntime} from './interactions';

export type PropKind = 'string' | 'boolean' | 'number' | 'string-array' | 'options' | 'columns' | 'rows' | 'collection-items' | 'record' | 'chips';

export type PropRule = {
  kind: PropKind;
  required?: boolean;
  values?: readonly string[];
};

export type ComponentDocumentation = {
  summary: string;
  children: 'none' | 'optional' | 'required';
  slots: readonly string[];
  keyboard: string;
  semantics: string;
};

export type HostRenderContext = {
  host: HostProfile;
  formFactor: 'wide' | 'compact';
  embedded: boolean;
  embeddedHeadingOffset: 0 | 2;
  locale: string;
  messages: Readonly<Record<string, string>>;
  resources: ReadonlyMap<string, CallerSafeResourceDescriptor>;
  resolveText: (node: UiNode, property: string, fallback: string) => string;
  interaction?: MetadataInteractionRuntime;
};

export type HostComponentDefinition = {
  id: string;
  hosts: readonly HostProfile[];
  enabled?: boolean;
  props: Readonly<Record<string, PropRule>>;
  documentation: ComponentDocumentation;
  render: (node: UiNode, children: ReactNode, context: HostRenderContext) => ReactNode;
};

export type ComponentValidationError = {
  property: string;
  message: string;
};

const forbiddenProperty = /(^|[-_])(color|colour|background|class|classname|css|style)([-_]|$)/i;

export class HostComponentRegistry {
  readonly #components = new Map<string, HostComponentDefinition>();

  register(definition: HostComponentDefinition) {
    validateDefinition(definition);
    if (this.#components.has(definition.id)) throw new Error(`Host component ${definition.id} is already registered.`);
    this.#components.set(definition.id, Object.freeze({...definition}));
    return this;
  }

  get(id: string) {
    return this.#components.get(id);
  }

  definitions() {
    return [...this.#components.values()].sort((left, right) => left.id.localeCompare(right.id));
  }

  validate(node: UiNode, definition: HostComponentDefinition): ComponentValidationError[] {
    const errors: ComponentValidationError[] = [];
    const props = node.props ?? {};
    for (const [property, value] of Object.entries(props)) {
      if (forbiddenProperty.test(property)) {
        errors.push({property, message: 'Raw visual styling is not permitted.'});
      } else if (isDeclaredLocalizationKey(property, definition.props)) {
        if (typeof value !== 'string') errors.push({property, message: 'Localization key must be a string.'});
      } else if (!definition.props[property]) {
        errors.push({property, message: 'Property is not declared by the host component.'});
      }
    }
    for (const property of Object.keys(node.bindings ?? {})) {
      if (forbiddenProperty.test(property)) {
        errors.push({property, message: 'Raw visual styling is not permitted.'});
      } else if (property === 'action' || property.endsWith('_action')) {
        errors.push({property, message: 'Action references cannot be data-bound.'});
      } else if (!definition.props[property]) {
        errors.push({property, message: 'Binding target is not declared by the host component.'});
      }
    }
    for (const [property, rule] of Object.entries(definition.props)) {
      const value = props[property];
      if (rule.required && value === undefined && props[`${property}_key`] === undefined) {
        errors.push({property, message: 'Required property is missing.'});
        continue;
      }
      if (value === undefined) continue;
      if (!matchesKind(value, rule.kind)) {
        errors.push({property, message: `Expected ${rule.kind}.`});
      } else if (rule.values && (typeof value !== 'string' || !rule.values.includes(value))) {
        errors.push({property, message: `Expected one of: ${rule.values.join(', ')}.`});
      }
    }
    const childCount = node.children?.length ?? 0;
    if (definition.documentation.children === 'none' && childCount > 0) {
      errors.push({property: 'children', message: 'This component does not accept children.'});
    }
    if (definition.documentation.children === 'required' && childCount === 0) {
      errors.push({property: 'children', message: 'This component requires child content.'});
    }
    return errors;
  }
}

function validateDefinition(definition: HostComponentDefinition) {
  if (!/^[a-z0-9]+([.-][a-z0-9-]+)+$/.test(definition.id)) throw new Error(`Host component ${definition.id} has an invalid stable ID.`);
  if (definition.hosts.length === 0 || new Set(definition.hosts).size !== definition.hosts.length) throw new Error(`Host component ${definition.id} must declare unique supported hosts.`);
  if (!definition.documentation.summary.trim() || !definition.documentation.keyboard.trim() || !definition.documentation.semantics.trim()) throw new Error(`Host component ${definition.id} has incomplete accessibility documentation.`);
  for (const [property, rule] of Object.entries(definition.props)) {
    if (forbiddenProperty.test(property)) throw new Error(`Host component ${definition.id} declares forbidden visual property ${property}.`);
    if (rule.values && (rule.kind !== 'string' || rule.values.length === 0)) throw new Error(`Host component ${definition.id} has an invalid value set for ${property}.`);
  }
}

function isDeclaredLocalizationKey(property: string, rules: Readonly<Record<string, PropRule>>) {
  if (!property.endsWith('_key')) return false;
  const target = property.slice(0, -4);
  return rules[target]?.kind === 'string';
}

function matchesKind(value: unknown, kind: PropKind) {
  if (kind === 'string') return typeof value === 'string';
  if (kind === 'boolean') return typeof value === 'boolean';
  if (kind === 'number') return typeof value === 'number' && Number.isFinite(value);
  if (kind === 'string-array') return Array.isArray(value) && value.every(item => typeof item === 'string');
  if (kind === 'options') return Array.isArray(value) && value.every(item => isRecord(item) && typeof item.value === 'string' && typeof item.label === 'string');
  if (kind === 'columns') return Array.isArray(value) && value.every(item => isRecord(item) && typeof item.key === 'string' && typeof item.label === 'string' && (item.format === undefined || ['text', 'email', 'phone', 'number', 'date', 'date-time', 'boolean', 'money', 'user'].includes(String(item.format))) && (item.sort === undefined || typeof item.sort === 'string'));
  if (kind === 'chips') return Array.isArray(value) && value.every(item => isRecord(item) && typeof item.id === 'string' && typeof item.label === 'string' && typeof item.action === 'string' && (item.parameter === undefined || typeof item.parameter === 'string') && (item.value === undefined || typeof item.value === 'string'));
  if (kind === 'collection-items') return Array.isArray(value) && value.every(isCollectionItem);
  if (kind === 'record') return isCollectionItem(value);
  return Array.isArray(value) && value.every(isRecord);
}

function isCollectionItem(value: unknown) {
  return isRecord(value)
    && typeof value.id === 'string'
    && typeof value.title === 'string'
    && (value.description === undefined || typeof value.description === 'string')
    && (value.archived === undefined || typeof value.archived === 'boolean')
    && (value.values === undefined || isRecord(value.values));
}

export function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

export function stringProp(node: UiNode, property: string, fallback = '') {
  const value = node.props?.[property];
  return typeof value === 'string' ? value : fallback;
}

export function booleanProp(node: UiNode, property: string, fallback = false) {
  const value = node.props?.[property];
  return typeof value === 'boolean' ? value : fallback;
}

export function numberProp(node: UiNode, property: string, fallback: number) {
  const value = node.props?.[property];
  return typeof value === 'number' && Number.isFinite(value) ? value : fallback;
}
