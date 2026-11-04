import {useState} from 'react';
import {MagnifyingGlass, Plus, SlidersHorizontal, Trash} from '@phosphor-icons/react';
import {Alert, Avatar, Badge, Button, EmptyState, IconButton, Select, Skeleton, TextField} from '../components/ui';
import {collectionRendererDataSources, collectionRendererFixture, collectionRendererResources, MetadataPageRenderer, rendererFixture, rendererMessages} from '../plugin-ui/renderer';
import './showcase.css';

export function UiShowcase() {
  const [showSaved, setShowSaved] = useState(false);
  const [loading, setLoading] = useState(false);

  const simulateSave = () => {
    setLoading(true);
    setShowSaved(false);
    window.setTimeout(() => { setLoading(false); setShowSaved(true); }, 650);
  };

  return <main className="showcase">
    <header className="showcase__header">
      <div>
        <p className="showcase__eyebrow">Limiara foundations · Issue 96</p>
        <h1>Accessible, quiet, and made for work.</h1>
        <p>Reusable visual foundations derived from the selected People workspace direction. Warm chalk surfaces, forest actions, editorial hierarchy, and honest semantic states.</p>
      </div>
      <Badge variant="selected">Light theme</Badge>
    </header>

    {showSaved && <Alert variant="success" title="Changes saved">The success state is announced without interrupting the next task.</Alert>}

    <section className="showcase__section" aria-labelledby="actions-title">
      <div className="showcase__section-heading"><span>01</span><div><h2 id="actions-title">Actions</h2><p>Clear hierarchy, comfortable targets, and visible focus.</p></div></div>
      <div className="showcase__surface">
        <div className="showcase__row">
          <Button onClick={simulateSave} loading={loading}><Plus aria-hidden="true" />Add person</Button>
          <Button variant="secondary">Open record</Button>
          <Button variant="quiet">Cancel</Button>
          <Button variant="danger"><Trash aria-hidden="true" />Archive</Button>
          <Button disabled>Unavailable</Button>
        </div>
        <div className="showcase__row">
          <IconButton label="Search" icon={<MagnifyingGlass aria-hidden="true" />} />
          <IconButton label="Filter records" variant="quiet" icon={<SlidersHorizontal aria-hidden="true" />} />
          <IconButton label="Delete record" variant="danger" icon={<Trash aria-hidden="true" />} />
          <IconButton label="Unavailable action" icon={<Plus aria-hidden="true" />} disabled />
          <button className="showcase__focus-example">Visible focus</button>
        </div>
      </div>
    </section>

    <section className="showcase__section" aria-labelledby="forms-title">
      <div className="showcase__section-heading"><span>02</span><div><h2 id="forms-title">Fields</h2><p>Labels, descriptions, validation, and disabled states stay explicit.</p></div></div>
      <div className="showcase__surface showcase__fields">
        <TextField label="Full name" description="Use the name this person prefers." defaultValue="Hannah Carter" />
        <TextField label="Email address" type="email" defaultValue="hannah@northstar.co.uk" error="Check the email address format." />
        <Select label="Organisation" defaultValue="northstar"><option value="northstar">Northstar Services Ltd</option><option value="riverview">Riverview Property Group</option></Select>
        <TextField label="Employee ID" defaultValue="NS-1047" disabled />
      </div>
    </section>

    <section className="showcase__section" aria-labelledby="identity-title">
      <div className="showcase__section-heading"><span>03</span><div><h2 id="identity-title">Identity & status</h2><p>Compact signals support scanning without relying on colour alone.</p></div></div>
      <div className="showcase__surface">
        <div className="showcase__row showcase__avatars">
          <Avatar name="Hannah Carter" size="large" />
          <Avatar name="Michael Jennings" />
          <Avatar name="Sara Lewis" />
          <Avatar name="Daniel Patel" />
          <Avatar name="Emily Thompson" size="small" />
        </div>
        <div className="showcase__row">
          <Badge>Draft</Badge><Badge variant="selected">Selected</Badge><Badge variant="success">Active</Badge><Badge variant="warning">Needs review</Badge><Badge variant="danger">Archived</Badge>
        </div>
      </div>
    </section>

    <section className="showcase__section" aria-labelledby="feedback-title">
      <div className="showcase__section-heading"><span>04</span><div><h2 id="feedback-title">Feedback & waiting</h2><p>Every system state explains what happened and what comes next.</p></div></div>
      <div className="showcase__feedback-grid">
        <Alert title="Workspace updated">Contacts are ready for this workspace.</Alert>
        <Alert variant="success" title="Person created">Hannah Carter is now available to the team.</Alert>
        <Alert variant="warning" title="Connection is slow">Your changes are safe. We’ll keep trying.</Alert>
        <Alert variant="danger" title="Couldn’t save changes">Review the highlighted fields and try again.</Alert>
      </div>
      <div className="showcase__surface showcase__state-grid">
        <EmptyState title="No people yet" description="Add the first person to start building this workspace’s directory." action={<Button size="small"><Plus aria-hidden="true" />Add person</Button>} />
        <div className="showcase__loading"><span>Loading person details</span><Skeleton lines={3} /><Skeleton lines={2} /></div>
      </div>
    </section>

    <section className="showcase__section showcase__section--full" aria-labelledby="metadata-title">
      <div className="showcase__section-heading"><span>05</span><div><h2 id="metadata-title">Metadata host renderer</h2><p>The same resolved fixture uses the maintained registry, semantic tokens, localization, and responsive composition rules.</p></div></div>
      <div className="showcase__metadata-frame">
        <MetadataPageRenderer page={rendererFixture} host="internal-web" messages={rendererMessages} announcement="Metadata fixture loaded." embedded />
      </div>
      <div className="showcase__metadata-frame">
        <MetadataPageRenderer dataSources={collectionRendererDataSources} page={collectionRendererFixture} resources={collectionRendererResources} host="internal-web" announcement="Collection metadata fixture loaded." embedded />
      </div>
    </section>

    <footer className="showcase__footer"><span>Keyboard first</span><span>WCAG 2.2 AA palette</span><span>Reduced motion ready</span><span>Phosphor icons</span></footer>
  </main>;
}
