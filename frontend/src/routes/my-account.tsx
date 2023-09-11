import { Form, useRouteLoaderData } from "react-router-dom";
import { UpdateUser } from "../../../bindings/UpdateUser";
import { User } from "../../../bindings/User";
import { updateMe } from "../api";
import Button from "../components/Button";
import Input from "../components/forms/Input";
import PageHeader from "../components/PageHeader";
import FormDataToJson from "../utils/FormDataToJson";

export async function action({ request }: { request: Request }) {
	const formData = await request.formData();
	const data = FormDataToJson<UpdateUser>(formData);
	return updateMe(data);
}

export default function MyAccount() {
	const { user } = useRouteLoaderData("app") as { user: User };

	return <>
		<PageHeader>My Account</PageHeader>
		<main className="flex-grow p-10">
			<Form method="post" action="/account" className="flex flex-col space-y-2 items-start mb-4">
				<label className="flex flex-col text-xs">
					<span>Name</span>
					<Input name="name" defaultValue={user.name} type="text" />
				</label>
				<label className="flex flex-col text-xs">
					<span>Email</span>
					<Input name="email" defaultValue={user.email} type="email" />
				</label>
				<label className="flex flex-col text-xs">
					<span>Password</span>
					<Input name="password" placeholder="Enter new password..." type="password" />
				</label>
				<Button>Update Account</Button>
			</Form>
			<Button varient="danger">Delete Account</Button>
		</main>
	</>
}
