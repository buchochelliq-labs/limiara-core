import React from 'react';
import {ObjectsApiError, type ValidationIssue} from './api';

/** A failed save as a summary message and the issues to list under it. */
export function issuesText(error: unknown, fallback: string): {message: string; issues: ValidationIssue[]} {
  if (error instanceof ObjectsApiError) {
    if (error.reason === 'invalid') return {message: 'Some values need attention.', issues: error.issues};
    if (error.code === 'objects.duplicate_api_name') return {message: 'That API name is already used. Choose another name.', issues: []};
    if (error.code === 'objects.object_referenced') return {message: 'Other objects have lookups to this one. Delete those fields first.', issues: []};
    if (error.code === 'objects.object_in_use') return {message: 'An automation creates records of this object. Change or delete it first.', issues: []};
    if (error.code === 'objects.field_in_use') return {message: 'A validation rule, formula field or automation uses this field. Change or delete it first.', issues: []};
    if (error.code === 'automations.duplicate_name') return {message: 'Another automation on this object has that name. Choose another name.', issues: []};
    if (error.code === 'rules.duplicate_name') return {message: 'Another rule on this object has that name. Choose another name.', issues: []};
    return {message: error.message, issues: []};
  }
  return {message: fallback, issues: []};
}

export function Issues({issues}: {issues: ValidationIssue[]}) {
  return issues.length ? <ul>{issues.map(issue => <li key={`${issue.field}-${issue.code}`}>{issue.field}: {issue.detail}</li>)}</ul> : null;
}
